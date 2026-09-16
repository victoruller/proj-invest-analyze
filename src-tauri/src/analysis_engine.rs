/// Módulo de cálculo de métricas financeiras para ações da B3.
///
/// Cada métrica tem uma função dedicada que retorna o valor calculado
/// e uma string JSON com os detalhes do cálculo (exibida como tooltip no frontend).
use anyhow::Result;
use chrono::{Datelike, TimeZone, Utc};
use serde_json::json;
use std::sync::Arc;

use crate::data_cache::DataCache;
use crate::models::{AppSettings, IncomeRow, MetricDetails, Situacao, StockSummary};
use crate::y_finance_getter::{fx_symbol, is_bdr_ticker};

const DAY: i64 = 86_400;

// ─── Entrada pública ──────────────────────────────────────────────────────────

pub fn compute_metrics(
    ticker: &str,
    display_name: Option<&str>,
    current_price: Option<f64>,
    last_fetched_at: Option<i64>,
    cache: &Arc<DataCache>,
    settings: &AppSettings,
) -> Result<StockSummary> {
    let r = settings.desired_return_rate;
    let min_liq = settings.min_daily_liquidity;
    let min_dy = settings.min_dividend_yield;

    // ── Moeda do balanço vs. moeda da cotação ─────────────────────────────────
    let units = Units::load(ticker, cache)?;

    // ── Bases TTM (na moeda do balanço) ───────────────────────────────────────
    let ni_ttm = calc_ttm(ticker, cache, |row: &IncomeRow| row.net_income)?;
    let revenue_ttm = calc_ttm(ticker, cache, |row: &IncomeRow| row.total_revenue)?;
    let op_ttm = calc_ttm(ticker, cache, |row: &IncomeRow| row.operating_income)?;
    let net_income_ttm = ni_ttm.as_ref().map(|t| t.value);
    let receita_ttm = revenue_ttm.as_ref().map(|t| t.value);
    let operating_income_ttm = op_ttm.as_ref().map(|t| t.value);

    // ── Taxa de crescimento g (compartilhada pelos dois P/VP justos) ─────────
    let growth = calc_growth(ticker, cache, settings)?;

    // ── Métricas atuais ───────────────────────────────────────────────────────
    let (liquidez_diaria, liquidez_detail) = calc_liquidez(ticker, cache, min_liq)?;
    let (pvp_atual, pvp_atual_detail) = calc_pvp_atual(ticker, current_price, &units, cache)?;
    let (roe_atual, roe_atual_detail) = calc_roe_ttm(ticker, ni_ttm.as_ref(), cache)?;
    let (dy_atual, dy_atual_detail) = calc_dy_ttm(ticker, current_price, cache)?;
    let (pvp_justo, pvp_justo_detail) = calc_pvp_justo(roe_atual, "ROE TTM", &growth, r);
    let (pvp_medio_1a, pvp_medio_1a_detail) = calc_pvp_medio_1a(ticker, &units, cache)?;

    // ── Médias históricas 5 anos ──────────────────────────────────────────────
    let (pvp_medio_5a, pvp_medio_5a_detail) = calc_pvp_medio_5a(ticker, &units, cache)?;
    let (roe_medio_5a, roe_medio_5a_detail) = calc_roe_medio_5a(ticker, cache)?;
    let (dy_medio_5a, dy_medio_5a_detail) = calc_dy_medio_5a(ticker, cache)?;
    let (pvp_justo_5a, pvp_justo_5a_detail) =
        calc_pvp_justo(roe_medio_5a, "ROE médio 5a", &growth, r);

    // ── Extras (VP/ação, dividendos TTM, g sustentável) ───────────────────────
    let vp_por_acao = cache
        .get_latest_balance_quarterly(ticker)?
        .and_then(|b| match (b.total_equity, b.shares_outstanding) {
            (Some(e), Some(s)) if e > 0.0 && s > 0 => Some(e / s as f64),
            _ => None,
        })
        .zip(units.share_fx())
        .map(|(vp, fx)| vp * fx);
    let ttm_divs = cache.get_ttm_dividends(ticker)?;
    let dividends_ttm =
        (!ttm_divs.is_empty()).then(|| ttm_divs.iter().map(|d| d.amount).sum::<f64>());
    let g_sgr_pct = calc_g_sgr(ticker, roe_atual, net_income_ttm, dividends_ttm, &units, cache)?;
    let g_usado_detail = growth_detail(&growth, settings.max_growth_rate, g_sgr_pct);

    // ── Demais métricas financeiras ───────────────────────────────────────────
    let receita_ttm_detail = ttm_detail(
        revenue_ttm.as_ref(),
        "Receita TTM = Σ(total_revenue, 4 trimestres consecutivos)",
    );
    let (margem_liquida, margem_liquida_detail) = calc_margem_liquida(net_income_ttm, receita_ttm)?;
    let (margem_operacional, margem_operacional_detail) =
        calc_margem_operacional(operating_income_ttm, receita_ttm)?;
    let (roic, roic_detail) = calc_roic(ticker, operating_income_ttm, cache)?;
    let (net_debt, net_debt_detail, divida_liquida_pl, divida_liquida_pl_detail,
         divida_liquida_ebit, divida_liquida_ebit_detail) =
        calc_net_debt_metrics(ticker, operating_income_ttm, cache)?;
    let (pl, pl_detail) = calc_pl(ticker, current_price, net_income_ttm, &units, cache)?;
    let (ev_ebit, ev_ebit_detail) =
        calc_ev_ebit(ticker, current_price, net_debt, operating_income_ttm, &units, cache)?;
    let flags = calc_flags(divida_liquida_ebit, roic, pvp_atual, pvp_justo);

    // ── Situações ─────────────────────────────────────────────────────────────
    // As duas comparam o preço de HOJE; só muda o ROE usado no P/VP justo
    // (TTM vs. média de 5 anos, menos sensível a um ano atípico).
    let blocker = units.blocker();
    let (situacao_atual, situacao_atual_reason) = explain_missing_pvp(
        classify(pvp_atual, pvp_justo, dy_atual, roe_atual, liquidez_diaria, min_liq, min_dy),
        pvp_atual,
        &blocker,
    );
    let (situacao_5a, situacao_5a_reason) = explain_missing_pvp(
        classify(pvp_atual, pvp_justo_5a, dy_medio_5a, roe_medio_5a, liquidez_diaria, min_liq, min_dy),
        pvp_atual,
        &blocker,
    );

    Ok(StockSummary {
        ticker: ticker.to_string(),
        display_name: display_name.map(str::to_string),
        current_price,
        liquidez_diaria,
        pvp_atual,
        roe_atual,
        dy_atual,
        pvp_justo,
        pvp_justo_5a,
        pvp_medio_1a,
        pvp_medio_5a,
        roe_medio_5a,
        dy_medio_5a,
        g_usado: Some(growth.g),
        g_fonte: Some(growth.fonte.to_string()),
        vp_por_acao,
        net_income_ttm: units.to_quote(net_income_ttm),
        dividends_ttm,
        receita_ttm: units.to_quote(receita_ttm),
        margem_liquida,
        margem_operacional,
        roic,
        net_debt: units.to_quote(net_debt),
        divida_liquida_pl,
        divida_liquida_ebit,
        pl,
        ev_ebit,
        flags,
        warnings: Vec::new(),
        situacao_atual,
        situacao_atual_reason,
        situacao_5a,
        situacao_5a_reason,
        last_fetched_at,
        details: MetricDetails {
            liquidez: liquidez_detail,
            pvp_atual: pvp_atual_detail,
            roe_atual: roe_atual_detail,
            dy_atual: dy_atual_detail,
            pvp_justo: pvp_justo_detail,
            pvp_justo_5a: pvp_justo_5a_detail,
            pvp_medio_1a: pvp_medio_1a_detail,
            pvp_medio_5a: pvp_medio_5a_detail,
            roe_medio_5a: roe_medio_5a_detail,
            dy_medio_5a: dy_medio_5a_detail,
            g_usado: g_usado_detail,
            receita_ttm: receita_ttm_detail,
            margem_liquida: margem_liquida_detail,
            margem_operacional: margem_operacional_detail,
            roic: roic_detail,
            net_debt: net_debt_detail,
            divida_liquida_pl: divida_liquida_pl_detail,
            divida_liquida_ebit: divida_liquida_ebit_detail,
            pl: pl_detail,
            ev_ebit: ev_ebit_detail,
        },
    })
}

// ─── 0. Moedas e conversão ────────────────────────────────────────────────────
//
// O Yahoo entrega alguns balanços em moeda diferente da cotação (PETR4/VALE3 em USD,
// BDRs em USD/TWD). Valores por ação do balanço só podem ser comparados com o preço
// depois de convertidos pelo câmbio. BDRs precisam ainda da proporção BDR/ação, obtida
// comparando as ações que o Yahoo reporta para o ticker (em BDRs) com as do balanço.
//
// Essa proporção NÃO vale para ações comuns: nelas o Yahoo conta só a classe cotada
// (PETR4 = 5,4 bi preferenciais contra 12,9 bi ON+PN no balanço), então ações seguem
// apenas com o câmbio.

struct Units {
    fin_currency: Option<String>,
    quote_currency: Option<String>,
    fx_symbol: Option<String>,
    /// Câmbio atual moeda do balanço → moeda da cotação (1.0 se iguais ou não verificadas)
    fx: Option<f64>,
    is_bdr: bool,
    /// Quantos BDRs equivalem a uma ação da empresa (ex.: AAPL34 ≈ 19,87)
    bdr_ratio: Option<f64>,
}

impl Units {
    fn load(ticker: &str, cache: &Arc<DataCache>) -> Result<Self> {
        let (fin_currency, quote_currency) = cache.get_currencies(ticker)?;
        let (symbol, fx) = match (&fin_currency, &quote_currency) {
            (Some(fin), Some(quote)) if !fin.eq_ignore_ascii_case(quote) => {
                let symbol = fx_symbol(fin, quote);
                let rate = cache
                    .get_last_n_candles(&symbol, 1)?
                    .last()
                    .map(|c| c.close)
                    .filter(|v| *v > 0.0);
                (Some(symbol), rate)
            }
            _ => (None, Some(1.0)),
        };

        let is_bdr = is_bdr_ticker(ticker);
        let bdr_ratio = if is_bdr {
            let stmt_shares = cache
                .get_latest_balance_quarterly(ticker)?
                .and_then(|b| b.shares_outstanding);
            match (cache.get_quote_shares(ticker)?, stmt_shares) {
                (Some(quote_shares), Some(shares)) if shares > 0 => {
                    Some(quote_shares as f64 / shares as f64)
                }
                _ => None,
            }
        } else {
            None
        };

        Ok(Self {
            fin_currency,
            quote_currency,
            fx_symbol: symbol,
            fx,
            is_bdr,
            bdr_ratio,
        })
    }

    /// Fator para levar valores POR AÇÃO do balanço à unidade da cotação
    fn share_fx(&self) -> Option<f64> {
        self.apply_bdr_ratio(self.fx?)
    }

    /// Em BDRs, o valor por ação vira valor por BDR
    fn apply_bdr_ratio(&self, fx: f64) -> Option<f64> {
        if self.is_bdr {
            Some(fx / self.bdr_ratio?)
        } else {
            Some(fx)
        }
    }

    /// Câmbio médio de um período (para P/VP de anos passados)
    fn year_share_fx(&self, cache: &Arc<DataCache>, from: i64, to: i64) -> Result<Option<f64>> {
        let Some(symbol) = &self.fx_symbol else {
            return Ok(self.share_fx());
        };
        let candles = cache.get_candles_between(symbol, from, to)?;
        if candles.is_empty() {
            return Ok(None);
        }
        let avg = candles.iter().map(|c| c.close).sum::<f64>() / candles.len() as f64;
        Ok(self.apply_bdr_ratio(avg))
    }

    /// Converte um valor total do balanço para a moeda da cotação (exibição)
    fn to_quote(&self, value: Option<f64>) -> Option<f64> {
        value.zip(self.fx).map(|(v, fx)| v * fx)
    }

    fn blocker(&self) -> Option<String> {
        if self.fx.is_none() {
            Some(format!(
                "Câmbio {} indisponível — atualize o ativo",
                self.fx_symbol.as_deref().unwrap_or("?")
            ))
        } else if self.is_bdr && self.bdr_ratio.is_none() {
            Some("BDR sem proporção BDR/ação (Yahoo não informou) — P/VP e P/L indisponíveis".into())
        } else {
            None
        }
    }

    fn describe(&self) -> String {
        let moeda = match (&self.fin_currency, &self.quote_currency, &self.fx_symbol, self.fx) {
            (Some(fin), Some(quote), Some(symbol), Some(fx)) => {
                format!("Balanço em {fin} convertido para {quote} a {fx:.4} ({symbol})")
            }
            (_, _, Some(symbol), None) => format!("Câmbio {symbol} indisponível"),
            (Some(_), Some(quote), None, _) => format!("Balanço e cotação em {quote}"),
            _ => "Moeda do balanço não verificada (atualize o ativo); assumida igual à da cotação"
                .into(),
        };
        match self.bdr_ratio {
            Some(ratio) => format!("{moeda}; {ratio:.2} BDRs por ação da empresa"),
            None => moeda,
        }
    }
}

fn blocked_detail(formula: &str, units: &Units) -> Option<String> {
    Some(
        json!({
            "formula": formula,
            "invalido": true,
            "motivo": units.blocker(),
            "conversao": units.describe()
        })
        .to_string(),
    )
}

fn explain_missing_pvp(
    (situacao, reason): (Situacao, String),
    pvp: Option<f64>,
    blocker: &Option<String>,
) -> (Situacao, String) {
    match (pvp, blocker) {
        (None, Some(b)) => (Situacao::DadosInsuficientes, b.clone()),
        _ => (situacao, reason),
    }
}

// ─── TTM (Trailing Twelve Months) ─────────────────────────────────────────────
//
// Soma dos 4 últimos trimestres quando são consecutivos e todos têm o campo.
// Caso contrário (trimestre faltando ou nulo), usa o último exercício anual recente
// — somar 4 trimestres esparsos distorce lucro, ROE e margens.

struct Ttm {
    value: f64,
    fonte: String,
    periodos: Vec<serde_json::Value>,
}

fn calc_ttm(
    ticker: &str,
    cache: &Arc<DataCache>,
    field: fn(&IncomeRow) -> Option<f64>,
) -> Result<Option<Ttm>> {
    let now = Utc::now().timestamp();

    let quarters = cache.get_quarterly_income(ticker, 4)?;
    let values: Vec<f64> = quarters.iter().filter_map(field).collect();
    let first_ts = quarters.first().and_then(|q| q.period_ts);
    let last_ts = quarters.last().and_then(|q| q.period_ts);
    let consecutive = match (first_ts, last_ts) {
        (Some(first), Some(last)) => last - first <= 300 * DAY && now - last <= 548 * DAY,
        _ => false,
    };

    if quarters.len() == 4 && values.len() == 4 && consecutive {
        return Ok(Some(Ttm {
            value: values.iter().sum(),
            fonte: "4 trimestres consecutivos".into(),
            periodos: quarters
                .iter()
                .map(|q| json!({"periodo": q.period_label, "valor": field(q)}))
                .collect(),
        }));
    }

    let annual = cache.get_annual_income(ticker, 1)?;
    if let Some(row) = annual.last() {
        let recent = row.period_ts.is_some_and(|ts| now - ts <= 548 * DAY);
        if let (Some(value), true) = (field(row), recent) {
            return Ok(Some(Ttm {
                value,
                fonte: format!(
                    "exercício anual {} (trimestres incompletos ou não consecutivos)",
                    row.period_label
                ),
                periodos: vec![json!({"periodo": row.period_label, "valor": value})],
            }));
        }
    }

    Ok(None)
}

fn ttm_detail(ttm: Option<&Ttm>, formula: &str) -> Option<String> {
    ttm.map(|t| {
        json!({
            "formula": formula,
            "valor": t.value,
            "fonte": t.fonte,
            "periodos": t.periodos,
            "nota": "Valores na moeda do balanço"
        })
        .to_string()
    })
}

// ─── 1. Liquidez Diária ───────────────────────────────────────────────────────
//
// Fórmula: média(Close × Volume) dos últimos 252 pregões
// Fonte: price_history
// Flag de alerta: valor < settings.min_daily_liquidity

fn calc_liquidez(
    ticker: &str,
    cache: &Arc<DataCache>,
    min_liq: f64,
) -> Result<(Option<f64>, Option<String>)> {
    let candles = cache.get_last_n_candles(ticker, 252)?;
    if candles.is_empty() {
        return Ok((None, None));
    }

    let daily_vols: Vec<f64> = candles
        .iter()
        .filter_map(|c| c.volume.map(|v| c.close * v as f64))
        .collect();

    if daily_vols.is_empty() {
        return Ok((None, None));
    }

    let avg = daily_vols.iter().sum::<f64>() / daily_vols.len() as f64;
    let flagged = avg < min_liq;

    let date_from = ts_to_date(candles.first().map(|c| c.ts));
    let date_to = ts_to_date(candles.last().map(|c| c.ts));

    let detail = json!({
        "formula": "média(Close × Volume)",
        "pregoes_usados": daily_vols.len(),
        "pregoes_solicitados": 252,
        "media_brl": round2(avg),
        "threshold_brl": min_liq,
        "flagged_baixa_liquidez": flagged,
        "periodo": format!("{} a {}", date_from, date_to),
        "nota": "252 pregões ≈ 12 meses de dados ajustados por splits"
    });

    Ok((Some(avg), Some(detail.to_string())))
}

// ─── 2. P/VP Atual ───────────────────────────────────────────────────────────
//
// Fórmula: preço_atual / (total_equity / shares_outstanding × câmbio)
// Fonte: fast_info (preço) + balance_quarterly (balanço mais recente)

fn calc_pvp_atual(
    ticker: &str,
    current_price: Option<f64>,
    units: &Units,
    cache: &Arc<DataCache>,
) -> Result<(Option<f64>, Option<String>)> {
    let formula = "P/VP = preço_atual / VP_por_ação";

    let price = match current_price {
        Some(p) if p > 0.0 => p,
        _ => return Ok((None, None)),
    };

    let balance = match cache.get_latest_balance_quarterly(ticker)? {
        Some(b) => b,
        None => return Ok((None, None)),
    };

    let equity = match balance.total_equity {
        Some(e) if e > 0.0 => e,
        _ => return Ok((None, None)),
    };

    let shares = match balance.shares_outstanding {
        Some(s) if s > 0 => s as f64,
        _ => return Ok((None, None)),
    };

    let Some(fx) = units.share_fx() else {
        return Ok((None, blocked_detail(formula, units)));
    };

    let book_value_per_share = equity / shares * fx;
    let pvp = price / book_value_per_share;

    let detail = json!({
        "formula": formula,
        "preco_atual": round4(price),
        "total_equity": equity,
        "shares_outstanding": shares,
        "conversao": units.describe(),
        "vp_por_acao": round4(book_value_per_share),
        "pvp": round4(pvp),
        "periodo_balanco": balance.period_label,
        "nota": "Balanço trimestral mais recente com equity preenchida"
    });

    Ok((Some(pvp), Some(detail.to_string())))
}

// ─── 3. ROE Atual (TTM) ───────────────────────────────────────────────────────
//
// Fórmula: lucro_líquido TTM / média(equity, 4 últimos trimestres)
// Fonte: income (TTM) + balance_quarterly

fn calc_roe_ttm(
    ticker: &str,
    ni_ttm: Option<&Ttm>,
    cache: &Arc<DataCache>,
) -> Result<(Option<f64>, Option<String>)> {
    let Some(ni) = ni_ttm else {
        return Ok((None, None));
    };

    let balance_rows = cache.get_quarterly_balance(ticker, 4)?;
    let equity_values: Vec<f64> = balance_rows.iter().filter_map(|r| r.total_equity).collect();

    if equity_values.is_empty() {
        return Ok((None, None));
    }

    let avg_equity = equity_values.iter().sum::<f64>() / equity_values.len() as f64;
    if avg_equity <= 0.0 {
        return Ok((None, None));
    }

    let roe = ni.value / avg_equity;

    let equity_detail: Vec<_> = balance_rows
        .iter()
        .map(|r| json!({"periodo": r.period_label, "equity": r.total_equity}))
        .collect();

    let detail = json!({
        "formula": "ROE = lucro_líquido TTM / média(equity 4 trimestres)",
        "lucro_liquido_ttm": ni.value,
        "fonte_lucro": ni.fonte,
        "lucro_por_periodo": ni.periodos,
        "media_equity": avg_equity,
        "roe_pct": round2(roe * 100.0),
        "trimestres_equity": equity_detail,
        "nota": "TTM = últimos 4 trimestres consecutivos; sem eles, usa o último exercício anual"
    });

    Ok((Some(roe), Some(detail.to_string())))
}

// ─── 4. DY Atual (TTM) ───────────────────────────────────────────────────────
//
// Fórmula: Σ(dividendos dos últimos 12 meses) / preço_atual
// Fonte: dividends (filtrado por data >= hoje - 365 dias) + fast_info (preço)
// Nota: dividendos já ajustados por splits pelo yfinance-rs

fn calc_dy_ttm(
    ticker: &str,
    current_price: Option<f64>,
    cache: &Arc<DataCache>,
) -> Result<(Option<f64>, Option<String>)> {
    let price = match current_price {
        Some(p) if p > 0.0 => p,
        _ => return Ok((None, None)),
    };

    let divs = cache.get_ttm_dividends(ticker)?;
    let total: f64 = divs.iter().map(|d| d.amount).sum();
    let dy = total / price;

    let div_list: Vec<_> = divs
        .iter()
        .map(|d| json!({"data": ts_to_date(Some(d.ts)), "valor": round4(d.amount)}))
        .collect();

    let detail = json!({
        "formula": "DY = Σ(dividendos 12 meses) / preço_atual",
        "preco_atual": round4(price),
        "total_dividendos_12m": round4(total),
        "dy_pct": round2(dy * 100.0),
        "dividendos": div_list,
        "quantidade": divs.len(),
        "nota": "Dividendos dos últimos 12 meses, já ajustados por splits"
    });

    Ok((Some(dy), Some(detail.to_string())))
}

// ─── 5. P/VP Justo (Gordon Growth Model) ─────────────────────────────────────
//
// Fórmula: P/VP_justo = (ROE - g) / (r - g)
// Calculado duas vezes: com ROE TTM (P/VP justo) e com ROE médio 5a (P/VP justo 5a).
// Onde:
//   g = CAGR do lucro líquido anual, limitado a [0, settings.max_growth_rate]
//       Fallback: settings.fallback_growth_rate se dados insuficientes ou lucros negativos
//   r = settings.desired_return_rate
//
// Condições inválidas (retorna None):
//   - ROE ≤ 0 (empresa com prejuízo)
//   - g ≥ r (denominador ≤ 0)

struct Growth {
    g: f64,
    fonte: &'static str,
    nota: String,
    historico: Vec<serde_json::Value>,
}

fn calc_growth(ticker: &str, cache: &Arc<DataCache>, settings: &AppSettings) -> Result<Growth> {
    let annual_income = cache.get_annual_income(ticker, 5)?;
    let net_incomes: Vec<f64> = annual_income.iter().filter_map(|r| r.net_income).collect();
    let historico = annual_income
        .iter()
        .map(|r| json!({"periodo": r.period_label, "lucro_liquido": r.net_income}))
        .collect();
    let (g, fonte, nota) = compute_g(
        &net_incomes,
        settings.fallback_growth_rate,
        settings.max_growth_rate,
    );
    Ok(Growth { g, fonte, nota, historico })
}

/// CAGR do lucro líquido anual limitado a [0, g_max]: no Gordon g é crescimento perpétuo,
/// e um CAGR de poucos anos próximo de r faz o P/VP justo explodir.
fn compute_g(net_incomes: &[f64], fallback: f64, g_max: f64) -> (f64, &'static str, String) {
    if net_incomes.len() < 2 || net_incomes.iter().any(|&v| v <= 0.0) {
        let note = if net_incomes.len() < 2 {
            "Dados anuais insuficientes (< 2 anos)".into()
        } else {
            "Lucro negativo em algum período — CAGR inválido".into()
        };
        return (fallback, "fallback", note);
    }

    let n = (net_incomes.len() - 1) as f64;
    let first = net_incomes[0];
    let last = net_incomes[net_incomes.len() - 1];
    let cagr = (last / first).powf(1.0 / n) - 1.0;

    let note = format!(
        "CAGR de {} anos: ({:.2} / {:.2})^(1/{:.0}) - 1 = {:.2}%",
        net_incomes.len() - 1,
        last,
        first,
        n,
        cagr * 100.0
    );

    let g = cagr.clamp(0.0, g_max.max(0.0));
    if g == cagr {
        (g, "cagr", note)
    } else {
        let limited = format!("{note}; limitado ao intervalo [0%, {:.2}%]", g_max * 100.0);
        (g, "cagr_limitado", limited)
    }
}

fn calc_pvp_justo(
    roe: Option<f64>,
    roe_label: &str,
    growth: &Growth,
    r: f64,
) -> (Option<f64>, Option<String>) {
    let formula = "P/VP_justo = (ROE - g) / (r - g)";

    let roe = match roe {
        Some(v) if v > 0.0 => v,
        _ => {
            let detail = json!({
                "formula": formula,
                "roe_usado": roe_label,
                "invalido": true,
                "motivo": format!("{roe_label} ≤ 0 ou indisponível")
            });
            return (None, Some(detail.to_string()));
        }
    };

    let g = growth.g;
    if roe <= g {
        let detail = json!({
            "formula": formula,
            "roe_usado": roe_label,
            "invalido": true,
            "motivo": "ROE ≤ g — o crescimento não se sustenta com o retorno atual (justo ≤ 0)",
            "roe": round4(roe),
            "g": round4(g),
            "g_fonte": growth.fonte
        });
        return (None, Some(detail.to_string()));
    }
    if g >= r {
        let detail = json!({
            "formula": formula,
            "roe_usado": roe_label,
            "invalido": true,
            "motivo": "g ≥ r — taxa de crescimento maior ou igual ao retorno desejado",
            "g": round4(g),
            "r": round4(r),
            "g_fonte": growth.fonte
        });
        return (None, Some(detail.to_string()));
    }

    let pvp_justo = (roe - g) / (r - g);

    let detail = json!({
        "formula": formula,
        "roe_usado": roe_label,
        "roe": round4(roe),
        "g": round4(g),
        "g_pct": round2(g * 100.0),
        "g_fonte": growth.fonte,
        "g_nota": growth.nota,
        "r": round4(r),
        "r_pct": round2(r * 100.0),
        "calculo": format!("({:.4} - {:.4}) / ({:.4} - {:.4})", roe, g, r, g),
        "pvp_justo": round4(pvp_justo),
        "historico_lucro": growth.historico
    });

    (Some(pvp_justo), Some(detail.to_string()))
}

fn growth_detail(growth: &Growth, g_max: f64, g_sgr_pct: Option<f64>) -> Option<String> {
    let formula = if growth.fonte == "fallback" {
        "g = fallback configurado pelo usuário"
    } else {
        "g = (lucro_último / lucro_primeiro)^(1/n) - 1  [CAGR], limitado a [0, g máximo]"
    };
    let detail = json!({
        "formula": formula,
        "g_pct": round2(growth.g * 100.0),
        "g_fonte": growth.fonte,
        "nota": growth.nota,
        "g_maximo_pct": round2(g_max * 100.0),
        "historico_lucro_anual": growth.historico,
        "anos_disponiveis": growth.historico.len(),
        "g_sgr_pct": g_sgr_pct
    });
    Some(detail.to_string())
}

/// g sustentável = ROE × (1 − payout); só informativo (tooltip de g)
fn calc_g_sgr(
    ticker: &str,
    roe_atual: Option<f64>,
    net_income_ttm: Option<f64>,
    dividends_ttm: Option<f64>,
    units: &Units,
    cache: &Arc<DataCache>,
) -> Result<Option<f64>> {
    let (Some(roe), Some(ni), Some(div_per_share), Some(fx)) =
        (roe_atual, net_income_ttm, dividends_ttm, units.share_fx())
    else {
        return Ok(None);
    };
    if ni <= 0.0 {
        return Ok(None);
    }
    let Some(shares) = cache
        .get_latest_balance_quarterly(ticker)?
        .and_then(|b| b.shares_outstanding)
    else {
        return Ok(None);
    };
    let total_dividends = div_per_share / fx * shares as f64;
    let payout = (total_dividends / ni).clamp(0.0, 1.0);
    Ok(Some(round2(roe * (1.0 - payout) * 100.0)))
}

// ─── 6. P/VP Médio 1 Ano ─────────────────────────────────────────────────────
//
// Fórmula: média(preços diários últimos 252 pregões) / VP_por_ação (balanço mais recente)
// Representa como o ativo tem sido precificado em relação ao VP no último ano.

fn calc_pvp_medio_1a(
    ticker: &str,
    units: &Units,
    cache: &Arc<DataCache>,
) -> Result<(Option<f64>, Option<String>)> {
    let formula = "P/VP médio 1a = média(preços diários 252 pregões) / VP_por_ação";

    let candles = cache.get_last_n_candles(ticker, 252)?;
    if candles.is_empty() {
        return Ok((None, None));
    }

    let balance = match cache.get_latest_balance_quarterly(ticker)? {
        Some(b) => b,
        None => return Ok((None, None)),
    };

    let equity = match balance.total_equity {
        Some(e) if e > 0.0 => e,
        _ => return Ok((None, None)),
    };
    let shares = match balance.shares_outstanding {
        Some(s) if s > 0 => s as f64,
        _ => return Ok((None, None)),
    };

    let Some(fx) = units.share_fx() else {
        return Ok((None, blocked_detail(formula, units)));
    };

    let prices: Vec<f64> = candles.iter().map(|c| c.close).collect();
    let avg_price = prices.iter().sum::<f64>() / prices.len() as f64;
    let book_value_per_share = equity / shares * fx;
    let pvp_medio = avg_price / book_value_per_share;

    let date_from = ts_to_date(candles.first().map(|c| c.ts));
    let date_to = ts_to_date(candles.last().map(|c| c.ts));

    let detail = json!({
        "formula": formula,
        "preco_medio_1a": round4(avg_price),
        "vp_por_acao": round4(book_value_per_share),
        "conversao": units.describe(),
        "pvp_medio_1a": round4(pvp_medio),
        "pregoes": prices.len(),
        "periodo": format!("{} a {}", date_from, date_to),
        "balanco_usado": balance.period_label,
        "nota": "VP fixo no balanço mais recente. Preços ajustados por splits."
    });

    Ok((Some(pvp_medio), Some(detail.to_string())))
}

// ─── 7. P/VP Médio 5 Anos (informativo) ──────────────────────────────────────
//
// Para cada ano disponível nos últimos 5:
//   P/VP_ano = média(preços diários do ano) / VP_por_ação (balanço anual × câmbio médio do ano)
// P/VP médio 5a = média dos P/VP anuais
//
// Mostra como o mercado precificou o ativo historicamente. Não entra na classificação.

fn calc_pvp_medio_5a(
    ticker: &str,
    units: &Units,
    cache: &Arc<DataCache>,
) -> Result<(Option<f64>, Option<String>)> {
    let formula = "P/VP médio 5a = média(P/VP anuais dos últimos 5 anos)";

    if units.share_fx().is_none() {
        return Ok((None, blocked_detail(formula, units)));
    }

    let balance_anual = cache.get_annual_balance(ticker, 5)?;
    if balance_anual.is_empty() {
        return Ok((None, None));
    }

    let mut annual_pvps: Vec<serde_json::Value> = vec![];
    let mut pvp_values: Vec<f64> = vec![];

    for bal in &balance_anual {
        let equity = match bal.total_equity {
            Some(e) if e > 0.0 => e,
            _ => continue,
        };
        let shares = match bal.shares_outstanding {
            Some(s) if s > 0 => s as f64,
            _ => continue,
        };

        // Determina o ano pelo period_ts ou pelo label
        let year = bal
            .period_ts
            .and_then(|ts| {
                Utc.timestamp_opt(ts, 0).single().map(|dt| dt.year())
            })
            .unwrap_or_else(|| {
                bal.period_label.parse::<i32>().unwrap_or(0)
            });

        if year == 0 {
            continue;
        }

        // Pega preços do ano calendário
        let year_start = Utc.with_ymd_and_hms(year, 1, 1, 0, 0, 0).single();
        let year_end = Utc.with_ymd_and_hms(year, 12, 31, 23, 59, 59).single();

        let (ys, ye) = match (year_start, year_end) {
            (Some(s), Some(e)) => (s.timestamp(), e.timestamp()),
            _ => continue,
        };

        let candles = cache.get_candles_between(ticker, ys, ye)?;
        if candles.is_empty() {
            continue;
        }

        let Some(fx) = units.year_share_fx(cache, ys, ye)? else {
            continue;
        };

        let book_per_share = equity / shares * fx;
        let avg_price = candles.iter().map(|c| c.close).sum::<f64>() / candles.len() as f64;
        let pvp_year = avg_price / book_per_share;
        pvp_values.push(pvp_year);

        annual_pvps.push(json!({
            "ano": year,
            "preco_medio": round4(avg_price),
            "vp_por_acao": round4(book_per_share),
            "cambio_medio": round4(fx),
            "pvp": round4(pvp_year),
            "pregoes": candles.len()
        }));
    }

    if pvp_values.is_empty() {
        return Ok((None, None));
    }

    let pvp_medio = pvp_values.iter().sum::<f64>() / pvp_values.len() as f64;

    let detail = json!({
        "formula": formula,
        "pvp_medio_5a": round4(pvp_medio),
        "anos_calculados": pvp_values.len(),
        "por_ano": annual_pvps,
        "conversao": units.describe(),
        "nota": "Informativo, não entra na classificação. Para cada ano: avg(preços do ano) / VP_por_ação (balanço anual × câmbio médio do ano)."
    });

    Ok((Some(pvp_medio), Some(detail.to_string())))
}

// ─── 8. ROE Médio 5 Anos ──────────────────────────────────────────────────────
//
// Para cada ano dos últimos 5:
//   ROE_ano = lucro_líquido_anual / equity_anual
// ROE médio 5a = média dos ROE anuais

fn calc_roe_medio_5a(
    ticker: &str,
    cache: &Arc<DataCache>,
) -> Result<(Option<f64>, Option<String>)> {
    let income = cache.get_annual_income(ticker, 5)?;
    let balance = cache.get_annual_balance(ticker, 5)?;

    if income.is_empty() || balance.is_empty() {
        return Ok((None, None));
    }

    let mut annual_roes: Vec<serde_json::Value> = vec![];
    let mut roe_values: Vec<f64> = vec![];

    for inc in &income {
        let net_income = match inc.net_income {
            Some(v) => v,
            None => continue,
        };

        // Tenta encontrar o balanço do mesmo período
        let bal = balance.iter().find(|b| b.period_label == inc.period_label)
            .or_else(|| {
                // Tenta por ano se period_ts disponível
                if let Some(inc_ts) = inc.period_ts {
                    let inc_year = Utc.timestamp_opt(inc_ts, 0)
                        .single()
                        .map(|dt| dt.year())
                        .unwrap_or(0);
                    balance.iter().find(|b| {
                        b.period_ts
                            .and_then(|ts| Utc.timestamp_opt(ts, 0).single())
                            .map(|dt| dt.year())
                            .unwrap_or(-1) == inc_year
                    })
                } else {
                    None
                }
            });

        let equity = match bal.and_then(|b| b.total_equity) {
            Some(e) if e > 0.0 => e,
            _ => continue,
        };

        let roe = net_income / equity;
        roe_values.push(roe);

        annual_roes.push(json!({
            "periodo": inc.period_label,
            "lucro_liquido": net_income,
            "equity": equity,
            "roe_pct": round2(roe * 100.0)
        }));
    }

    if roe_values.is_empty() {
        return Ok((None, None));
    }

    let roe_medio = roe_values.iter().sum::<f64>() / roe_values.len() as f64;

    let detail = json!({
        "formula": "ROE médio 5a = média(ROE anuais dos últimos 5 anos)",
        "roe_medio_pct": round2(roe_medio * 100.0),
        "anos_calculados": roe_values.len(),
        "por_ano": annual_roes,
        "nota": "ROE_ano = lucro_líquido_anual / equity_anual (balanço anual)"
    });

    Ok((Some(roe_medio), Some(detail.to_string())))
}

// ─── 9. DY Médio 5 Anos ──────────────────────────────────────────────────────
//
// Para cada uma das 5 janelas de 12 meses terminando hoje:
//   DY_janela = Σ(dividendos da janela) / preço_médio_da_janela
// DY médio 5a = média dos DY das janelas
//
// Janelas móveis evitam contar o ano corrente pela metade (que subestimaria o DY).

fn calc_dy_medio_5a(
    ticker: &str,
    cache: &Arc<DataCache>,
) -> Result<(Option<f64>, Option<String>)> {
    let all_dividends = cache.get_all_dividends(ticker)?;
    let now = Utc::now().timestamp();

    let mut janelas: Vec<serde_json::Value> = vec![];
    let mut dy_values: Vec<f64> = vec![];

    for k in 0..5 {
        let end = now - k * 365 * DAY;
        let start = end - 365 * DAY;

        let candles = cache.get_candles_between(ticker, start + 1, end)?;
        // ~248 pregões por ano; janelas com histórico parcial distorcem preço médio e dividendos
        if candles.len() < 200 {
            continue;
        }

        let avg_price = candles.iter().map(|c| c.close).sum::<f64>() / candles.len() as f64;
        if avg_price <= 0.0 {
            continue;
        }

        let divs: Vec<f64> = all_dividends
            .iter()
            .filter(|d| d.ts > start && d.ts <= end)
            .map(|d| d.amount)
            .collect();
        let total_divs: f64 = divs.iter().sum();

        let dy = total_divs / avg_price;
        dy_values.push(dy);

        janelas.push(json!({
            "janela": format!("{} a {}", ts_to_date(Some(start)), ts_to_date(Some(end))),
            "total_dividendos": round4(total_divs),
            "preco_medio": round4(avg_price),
            "dy_pct": round2(dy * 100.0),
            "num_dividendos": divs.len()
        }));
    }

    if dy_values.is_empty() {
        return Ok((None, None));
    }

    let dy_medio = dy_values.iter().sum::<f64>() / dy_values.len() as f64;

    let detail = json!({
        "formula": "DY médio 5a = média(DY de 5 janelas de 12 meses)",
        "dy_medio_pct": round2(dy_medio * 100.0),
        "janelas_calculadas": dy_values.len(),
        "por_janela": janelas,
        "nota": "DY_janela = Σ(dividendos em 12 meses) / preço médio da janela. Janelas com menos de 200 pregões são ignoradas."
    });

    Ok((Some(dy_medio), Some(detail.to_string())))
}

// ─── Novas métricas financeiras ───────────────────────────────────────────────

/// Margem Líquida = net_income_ttm / receita_ttm
fn calc_margem_liquida(
    net_income_ttm: Option<f64>,
    receita_ttm: Option<f64>,
) -> Result<(Option<f64>, Option<String>)> {
    let ni = match net_income_ttm { Some(v) => v, None => return Ok((None, None)) };
    let rev = match receita_ttm { Some(v) if v != 0.0 => v, _ => return Ok((None, None)) };
    let margem = ni / rev;
    let detail = json!({
        "formula": "Margem Líquida = net_income_ttm / receita_ttm",
        "net_income_ttm": ni,
        "receita_ttm": rev,
        "margem_pct": round2(margem * 100.0)
    });
    Ok((Some(margem), Some(detail.to_string())))
}

/// Margem Operacional = operating_income_ttm / receita_ttm
fn calc_margem_operacional(
    operating_income_ttm: Option<f64>,
    receita_ttm: Option<f64>,
) -> Result<(Option<f64>, Option<String>)> {
    let op = match operating_income_ttm { Some(v) => v, None => return Ok((None, None)) };
    let rev = match receita_ttm { Some(v) if v != 0.0 => v, _ => return Ok((None, None)) };
    let margem = op / rev;
    let detail = json!({
        "formula": "Margem Operacional (EBIT) = operating_income_ttm / receita_ttm",
        "operating_income_ttm": op,
        "receita_ttm": rev,
        "margem_pct": round2(margem * 100.0),
        "nota": "Proxy de EBIT margin (D&A não disponível via yfinance-rs)"
    });
    Ok((Some(margem), Some(detail.to_string())))
}

/// ROIC = NOPAT / invested_capital
/// NOPAT = operating_income_ttm × 0.66 (34% BR tax)
/// invested_capital = total_equity + long_term_debt - cash
fn calc_roic(
    ticker: &str,
    operating_income_ttm: Option<f64>,
    cache: &Arc<DataCache>,
) -> Result<(Option<f64>, Option<String>)> {
    let op_income = match operating_income_ttm { Some(v) => v, None => return Ok((None, None)) };
    let balance = match cache.get_latest_balance_quarterly(ticker)? {
        Some(b) => b,
        None => return Ok((None, None)),
    };
    let equity = match balance.total_equity { Some(v) => v, None => return Ok((None, None)) };
    let long_term_debt = balance.long_term_debt.unwrap_or(0.0);
    let cash = balance.cash.unwrap_or(0.0);

    let nopat = op_income * 0.66;
    let invested_capital = equity + long_term_debt - cash;

    if invested_capital <= 0.0 {
        let detail = json!({
            "formula": "ROIC = NOPAT / invested_capital",
            "invalido": true,
            "motivo": "Capital investido ≤ 0",
            "nopat": nopat,
            "invested_capital": invested_capital
        });
        return Ok((None, Some(detail.to_string())));
    }

    let roic = nopat / invested_capital;
    let detail = json!({
        "formula": "ROIC = NOPAT / (equity + long_term_debt - cash)",
        "operating_income_ttm": op_income,
        "taxa_imposto": "34% (BR)",
        "nopat": nopat,
        "total_equity": equity,
        "long_term_debt": long_term_debt,
        "cash": cash,
        "invested_capital": invested_capital,
        "roic_pct": round2(roic * 100.0),
        "periodo_balanco": balance.period_label
    });
    Ok((Some(roic), Some(detail.to_string())))
}

/// Dívida Líquida e métricas derivadas
fn calc_net_debt_metrics(
    ticker: &str,
    operating_income_ttm: Option<f64>,
    cache: &Arc<DataCache>,
) -> Result<(Option<f64>, Option<String>, Option<f64>, Option<String>, Option<f64>, Option<String>)> {
    let balance = match cache.get_latest_balance_quarterly(ticker)? {
        Some(b) => b,
        None => return Ok((None, None, None, None, None, None)),
    };

    let long_term_debt = balance.long_term_debt.unwrap_or(0.0);
    let cash = balance.cash.unwrap_or(0.0);
    let net_debt = long_term_debt - cash;

    let net_debt_detail = json!({
        "formula": "Dívida Líquida = long_term_debt - cash",
        "long_term_debt": long_term_debt,
        "cash": cash,
        "net_debt": net_debt,
        "periodo_balanco": balance.period_label,
        "nota": "Negativo indica posição de caixa líquida. Valores na moeda do balanço."
    });

    // D/PL
    let (divida_pl, divida_pl_detail) = if let Some(equity) = balance.total_equity {
        if equity != 0.0 {
            let ratio = net_debt / equity;
            let detail = json!({
                "formula": "D.Líq./P.L. = net_debt / total_equity",
                "net_debt": net_debt,
                "total_equity": equity,
                "ratio": round2(ratio)
            });
            (Some(ratio), Some(detail.to_string()))
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    // D/EBIT
    let (divida_ebit, divida_ebit_detail) = if let Some(op) = operating_income_ttm {
        if op != 0.0 {
            let ratio = net_debt / op;
            let detail = json!({
                "formula": "D.Líq./EBIT = net_debt / operating_income_ttm",
                "net_debt": net_debt,
                "operating_income_ttm": op,
                "ratio": round2(ratio),
                "nota": "Proxy de D/EBITDA (D&A não disponível via yfinance-rs)"
            });
            (Some(ratio), Some(detail.to_string()))
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    Ok((
        Some(net_debt),
        Some(net_debt_detail.to_string()),
        divida_pl,
        divida_pl_detail,
        divida_ebit,
        divida_ebit_detail,
    ))
}

/// P/L = preço / (net_income_ttm / shares × câmbio)
fn calc_pl(
    ticker: &str,
    current_price: Option<f64>,
    net_income_ttm: Option<f64>,
    units: &Units,
    cache: &Arc<DataCache>,
) -> Result<(Option<f64>, Option<String>)> {
    let formula = "P/L = preço / LPA = preço / (net_income_ttm / shares)";

    let price = match current_price { Some(p) if p > 0.0 => p, _ => return Ok((None, None)) };
    let ni = match net_income_ttm { Some(v) => v, None => return Ok((None, None)) };

    let balance = match cache.get_latest_balance_quarterly(ticker)? {
        Some(b) => b,
        None => return Ok((None, None)),
    };
    let shares = match balance.shares_outstanding {
        Some(s) if s > 0 => s as f64,
        _ => return Ok((None, None)),
    };

    if ni <= 0.0 {
        let detail = json!({
            "formula": formula,
            "invalido": true,
            "motivo": "Lucro líquido TTM ≤ 0",
            "net_income_ttm": ni
        });
        return Ok((None, Some(detail.to_string())));
    }

    let Some(fx) = units.share_fx() else {
        return Ok((None, blocked_detail(formula, units)));
    };

    let eps = ni / shares * fx;
    let pl = price / eps;

    let detail = json!({
        "formula": formula,
        "preco_atual": round4(price),
        "net_income_ttm": ni,
        "shares": shares,
        "conversao": units.describe(),
        "eps": round4(eps),
        "pl": round2(pl),
        "periodo_balanco": balance.period_label
    });
    Ok((Some(pl), Some(detail.to_string())))
}

/// EV/EBIT = (market_cap + net_debt) / operating_income_ttm, tudo na moeda do balanço
fn calc_ev_ebit(
    ticker: &str,
    current_price: Option<f64>,
    net_debt: Option<f64>,
    operating_income_ttm: Option<f64>,
    units: &Units,
    cache: &Arc<DataCache>,
) -> Result<(Option<f64>, Option<String>)> {
    let formula = "EV/EBIT = (market_cap + net_debt) / operating_income_ttm";

    let price = match current_price { Some(p) if p > 0.0 => p, _ => return Ok((None, None)) };
    let op = match operating_income_ttm { Some(v) if v != 0.0 => v, _ => return Ok((None, None)) };
    let nd = net_debt.unwrap_or(0.0);

    let balance = match cache.get_latest_balance_quarterly(ticker)? {
        Some(b) => b,
        None => return Ok((None, None)),
    };
    let shares = match balance.shares_outstanding {
        Some(s) if s > 0 => s as f64,
        _ => return Ok((None, None)),
    };

    let Some(fx) = units.share_fx() else {
        return Ok((None, blocked_detail(formula, units)));
    };

    let market_cap = price * shares / fx;
    let ev = market_cap + nd;
    let ev_ebit = ev / op;

    let detail = json!({
        "formula": formula,
        "preco_atual": round4(price),
        "shares": shares,
        "conversao": units.describe(),
        "market_cap": market_cap,
        "net_debt": nd,
        "ev": ev,
        "operating_income_ttm": op,
        "ev_ebit": round2(ev_ebit),
        "nota": "Valores na moeda do balanço. D&A não disponível via yfinance-rs — usar como proxy de EV/EBITDA com cautela",
        "periodo_balanco": balance.period_label
    });
    Ok((Some(ev_ebit), Some(detail.to_string())))
}

/// Gera flags de alerta/oportunidade
fn calc_flags(
    divida_liquida_ebit: Option<f64>,
    roic: Option<f64>,
    pvp_atual: Option<f64>,
    pvp_justo: Option<f64>,
) -> Vec<String> {
    let mut flags = Vec::new();

    if let Some(d) = divida_liquida_ebit {
        if d > 3.0 {
            flags.push("🚩 Endividamento alto (D/EBIT > 3.0)".to_string());
        }
    }

    if let (Some(r), Some(pvp_a), Some(pvp_j)) = (roic, pvp_atual, pvp_justo) {
        if r > 0.15 && pvp_a < pvp_j {
            flags.push("⭐ Eficiente e subavaliada (ROIC > 15% + subavaliada)".to_string());
        }
    }

    flags
}

// ─── Classificação de Situação ────────────────────────────────────────────────
//
// Usada para as duas situações, sempre com o P/VP ATUAL (preço de hoje):
//   situacao_atual → P/VP justo (ROE TTM),      DY TTM,      ROE TTM
//   situacao_5a    → P/VP justo 5a (ROE méd. 5a), DY médio 5a, ROE médio 5a
//
// Regras (em ordem de prioridade):
//   ⚠️ DADOS_INSUFICIENTES: PVP ou ROE ausentes
//   🔴 VENDA:
//       - ROE < 0 (prejuízo)
//       - Liquidez < 50% do threshold (muito baixa)
//       - PVP > pvp_justo × 1.2 (sobreavaliada em 20%+)
//   🟢 COMPRA (todos devem ser verdadeiros):
//       - PVP < pvp_justo × 0.9 (subavaliada em 10%+)
//       - Liquidez ≥ threshold
//       - ROE ≥ 10%
//       - DY ≥ settings.min_dividend_yield (padrão 0 = sem filtro)
//
// O DY não é exigido por padrão: (ROE - g)/(r - g) já embute o dividendo, pois o modelo
// assume payout de 1 - g/ROE. Exigir DY em cima disso contava o dividendo duas vezes e
// barrava empresas de crescimento.
//   🟡 MANTER: demais casos

fn classify(
    pvp: Option<f64>,
    pvp_justo: Option<f64>,
    dy: Option<f64>,
    roe: Option<f64>,
    liquidez: Option<f64>,
    min_liq: f64,
    min_dy: f64,
) -> (Situacao, String) {
    let pvp = match pvp {
        Some(v) => v,
        None => return (Situacao::DadosInsuficientes, "P/VP não calculável".into()),
    };
    let roe = match roe {
        Some(v) => v,
        None => return (Situacao::DadosInsuficientes, "ROE não calculável".into()),
    };
    let liq = liquidez.unwrap_or(0.0);
    let dy_val = dy.unwrap_or(0.0);

    // VENDA
    if roe < 0.0 {
        return (Situacao::Venda, "ROE negativo (prejuízo recente)".into());
    }
    if liq < min_liq * 0.5 {
        return (Situacao::Venda, "Liquidez muito baixa".into());
    }
    if let Some(pj) = pvp_justo {
        if pvp > pj * 1.2 {
            return (
                Situacao::Venda,
                format!("Sobreavaliada (P/VP {:.2}x > P/VP justo {:.2}x × 1.2)", pvp, pj),
            );
        }
    }

    // COMPRA (todos os critérios)
    let liq_ok = liq >= min_liq;
    let roe_ok = roe >= 0.10;
    let dy_ok = dy_val >= min_dy;

    if let Some(pj) = pvp_justo {
        if pvp < pj * 0.9 && liq_ok && roe_ok && dy_ok {
            let mut reasons = vec!["Subavaliada"];
            if min_dy > 0.0 { reasons.push("DY acima do mínimo"); }
            return (Situacao::Compra, reasons.join(" + "));
        }
        // MANTER: PVP dentro de ±10% do justo
        if pvp >= pj * 0.9 && pvp <= pj * 1.1 {
            return (Situacao::Manter, "P/VP próximo ao justo".into());
        }
    }

    // Sinais mistos ou pvp_justo indisponível
    if liq_ok && roe_ok {
        (Situacao::Manter, "Fundamentos saudáveis — sinais mistos de valuation".into())
    } else if !liq_ok {
        (Situacao::Manter, "Liquidez baixa".into())
    } else {
        (Situacao::Manter, "Sinais mistos".into())
    }
}

// ─── Utilitários ──────────────────────────────────────────────────────────────

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn round4(v: f64) -> f64 {
    (v * 10000.0).round() / 10000.0
}

fn ts_to_date(ts: Option<i64>) -> String {
    ts.and_then(|t| Utc.timestamp_opt(t, 0).single())
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "N/A".into())
}
