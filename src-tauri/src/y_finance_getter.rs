use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use yfinance_rs::core::conversions::money_to_f64;
use yfinance_rs::core::Candle;
use yfinance_rs::fundamentals::{BalanceSheetRow, CashflowRow as YfCashflowRow, IncomeStatementRow};
use yfinance_rs::{FastInfo, Interval, Range, Ticker, YfClient, YfError};

use crate::models::{BalanceRow, CandleRow, CashflowRow, DividendRow, IncomeRow};

const TIMESERIES_URL: &str =
    "https://query2.finance.yahoo.com/ws/fundamentals-timeseries/v1/finance/timeseries/";
const QUOTE_SUMMARY_URL: &str = "https://query1.finance.yahoo.com/v10/finance/quoteSummary/";
const YAHOO_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                        (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

/// Sessão autenticada (cookie + crumb), negociada uma única vez por execução
static YAHOO_SESSION: tokio::sync::OnceCell<Option<(reqwest::Client, String)>> =
    tokio::sync::OnceCell::const_new();

// ─── Helpers ──────────────────────────────────────────────────────────────────

// Preços são coletados com auto_adjust(false): ajuste por splits sim, por dividendos não.
// Com ajuste por dividendos o fechamento da PETR4 em jan/2022 vira 9,66 em vez de 29,09,
// o que deflaciona o passado e triplica o DY histórico.

/// Adiciona sufixo .SA para tickers da B3 (ex: "VALE3" -> "VALE3.SA")
pub fn b3_symbol(ticker: &str) -> String {
    if ticker.ends_with(".SA") || ticker.ends_with(".sa") {
        ticker.to_uppercase()
    } else {
        format!("{}.SA", ticker.to_uppercase())
    }
}

/// BDRs na B3 terminam em 31–35 ou 39 (ex.: AAPL34); ações terminam em 3–8 e units em 11.
pub fn is_bdr_ticker(ticker: &str) -> bool {
    let code = ticker.trim_end_matches(".SA").as_bytes();
    code.len() >= 6 && matches!(code[code.len() - 2..], [b'3', b'1'..=b'5' | b'9'])
}

/// Símbolo do par de câmbio no Yahoo (ex: USD → BRL = "USDBRL=X")
pub fn fx_symbol(from: &str, to: &str) -> String {
    format!("{}{}=X", from.to_uppercase(), to.to_uppercase())
}

/// Cria um cliente Yahoo Finance com configuração padrão
pub fn build_client() -> YfClient {
    YfClient::default()
}

// ─── Conversores de tipos yfinance-rs → models ────────────────────────────────

/// Converte o string canônico de Period em timestamp Unix.
/// Formatos: "2024Q3" (trimestre), "2024" (ano), "2024-03-15" (data)
fn period_to_ts(period_str: &str) -> Option<i64> {
    // Trimestral: "2024Q3"
    if let Some(q_pos) = period_str.find('Q') {
        let year: i32 = period_str[..q_pos].parse().ok()?;
        let quarter: u8 = period_str[q_pos + 1..].parse().ok()?;
        let (month, day) = match quarter {
            1 => (3u32, 31u32),
            2 => (6, 30),
            3 => (9, 30),
            _ => (12, 31),
        };
        return chrono::NaiveDate::from_ymd_opt(year, month, day)
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|dt| Utc.from_utc_datetime(&dt).timestamp());
    }

    // Data completa: "2024-03-15"
    if period_str.len() == 10 && period_str.chars().nth(4) == Some('-') {
        return chrono::NaiveDate::parse_from_str(period_str, "%Y-%m-%d")
            .ok()
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|dt| Utc.from_utc_datetime(&dt).timestamp());
    }

    // Anual: "2024"
    if let Ok(year) = period_str.parse::<i32>() {
        return chrono::NaiveDate::from_ymd_opt(year, 12, 31)
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|dt| Utc.from_utc_datetime(&dt).timestamp());
    }

    None
}

fn income_row_from(row: &IncomeStatementRow) -> IncomeRow {
    let label = row.period.to_string();
    IncomeRow {
        period_ts: period_to_ts(&label),
        period_label: label,
        net_income: row.net_income.as_ref().map(money_to_f64),
        total_revenue: row.total_revenue.as_ref().map(money_to_f64),
        operating_income: row.operating_income.as_ref().map(money_to_f64),
    }
}

fn balance_row_from(row: &BalanceSheetRow) -> BalanceRow {
    let label = row.period.to_string();
    BalanceRow {
        period_ts: period_to_ts(&label),
        period_label: label,
        total_equity: row.total_equity.as_ref().map(money_to_f64),
        shares_outstanding: row.shares_outstanding,
        cash: row.cash.as_ref().map(money_to_f64),
        long_term_debt: row.long_term_debt.as_ref().map(money_to_f64),
        total_assets: row.total_assets.as_ref().map(money_to_f64),
        total_liabilities: row.total_liabilities.as_ref().map(money_to_f64),
    }
}

fn cashflow_row_from(row: &YfCashflowRow) -> CashflowRow {
    let label = row.period.to_string();
    CashflowRow {
        period_ts: period_to_ts(&label),
        period_label: label,
        operating_cashflow: row.operating_cashflow.as_ref().map(money_to_f64),
        free_cash_flow: row.free_cash_flow.as_ref().map(money_to_f64),
    }
}

fn candle_row_from(c: &Candle) -> CandleRow {
    CandleRow {
        ts: c.ts.timestamp(),
        open: money_to_f64(&c.open),
        high: money_to_f64(&c.high),
        low: money_to_f64(&c.low),
        close: money_to_f64(&c.close),
        volume: c.volume,
    }
}

/// Falha parcial não aborta a coleta, mas vira aviso em vez de sumir silenciosamente
fn take<T: Default>(res: Result<T, YfError>, label: &str, warnings: &mut Vec<String>) -> T {
    match res {
        Ok(value) => value,
        Err(e) => {
            warnings.push(format!("{label}: {e}"));
            T::default()
        }
    }
}

/// (nome, preço atual, moeda da cotação)
fn quote_info(
    res: Result<FastInfo, YfError>,
    warnings: &mut Vec<String>,
) -> (Option<String>, Option<f64>, Option<String>) {
    match res {
        Ok(fi) => {
            let price = fi.last.as_ref().map(money_to_f64);
            if price.is_none() {
                warnings.push("Cotação atual indisponível: resposta sem preço".into());
            }
            (fi.name, price, fi.currency.map(|c| c.to_string()))
        }
        Err(e) => {
            warnings.push(format!("Cotação atual indisponível: {e}"));
            (None, None, None)
        }
    }
}

// ─── Moedas ───────────────────────────────────────────────────────────────────

/// Moeda real dos balanços: o yfinance-rs a infere pelo país (PETR4 → BRL), mas o Yahoo
/// entrega PETR4/VALE3 em USD. O endpoint de timeseries traz o `currencyCode` de cada ponto.
async fn fetch_statement_currency(symbol: &str) -> Option<String> {
    let now = Utc::now().timestamp();
    let period1 = (now - 3 * 365 * 86_400).to_string();
    let period2 = now.to_string();

    let body: serde_json::Value = reqwest::Client::new()
        .get(format!("{TIMESERIES_URL}{symbol}"))
        .query(&[
            ("symbol", symbol),
            ("type", "quarterlyStockholdersEquity,annualNetIncome"),
            ("period1", period1.as_str()),
            ("period2", period2.as_str()),
        ])
        .header(reqwest::header::USER_AGENT, "Mozilla/5.0")
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;

    body["timeseries"]["result"]
        .as_array()?
        .iter()
        .filter_map(|series| {
            let kind = series["meta"]["type"][0].as_str()?;
            series[kind].as_array()
        })
        .flatten()
        .filter_map(|point| Some((point["asOfDate"].as_str()?, point["currencyCode"].as_str()?)))
        .max_by_key(|&(date, _)| date)
        .map(|(_, currency)| currency.to_uppercase())
}

/// Cookie + crumb do Yahoo, mesmo fluxo do yfinance: `fc.yahoo.com` grava o cookie
/// (responde 404, o que é esperado) e só então `getcrumb` aceita a sessão.
async fn yahoo_session() -> Option<&'static (reqwest::Client, String)> {
    YAHOO_SESSION
        .get_or_init(|| async {
            let client = reqwest::Client::builder()
                .cookie_store(true)
                .user_agent(YAHOO_UA)
                .build()
                .ok()?;

            let _ = client.get("https://fc.yahoo.com/").send().await;

            let crumb = client
                .get("https://query1.finance.yahoo.com/v1/test/getcrumb")
                .send()
                .await
                .ok()?
                .text()
                .await
                .ok()?;

            if crumb.trim().is_empty() || crumb.contains("<html") || crumb.contains("Invalid") {
                return None;
            }
            Some((client, crumb))
        })
        .await
        .as_ref()
}

/// Ações em circulação na unidade do ticker cotado. Para BDRs o Yahoo devolve o total em
/// BDRs, o que, comparado às ações do balanço, revela a proporção BDR/ação.
async fn fetch_quote_shares(symbol: &str, warnings: &mut Vec<String>) -> Option<u64> {
    let Some((client, crumb)) = yahoo_session().await else {
        warnings.push("Sessão do Yahoo indisponível — proporção BDR/ação não obtida".into());
        return None;
    };

    let response = client
        .get(format!("{QUOTE_SUMMARY_URL}{symbol}"))
        .query(&[
            ("modules", "defaultKeyStatistics"),
            ("crumb", crumb.as_str()),
        ])
        .send()
        .await
        .and_then(reqwest::Response::error_for_status);

    let body: serde_json::Value = match response {
        Ok(resp) => match resp.json().await {
            Ok(value) => value,
            Err(e) => {
                warnings.push(format!("Proporção BDR/ação: resposta ilegível ({e})"));
                return None;
            }
        },
        Err(e) => {
            warnings.push(format!("Proporção BDR/ação indisponível: {e}"));
            return None;
        }
    };

    let shares = body["quoteSummary"]["result"][0]["defaultKeyStatistics"]["sharesOutstanding"]
        ["raw"]
        .as_u64()
        .filter(|s| *s > 0);
    if shares.is_none() {
        warnings.push("Proporção BDR/ação: Yahoo não informou ações em circulação".into());
    }
    shares
}

/// Histórico de câmbio (5 anos) quando o balanço está em moeda diferente da cotação
async fn fetch_fx_if_needed(
    client: &YfClient,
    fin_currency: Option<&str>,
    quote_currency: Option<&str>,
    warnings: &mut Vec<String>,
) -> Option<(String, Vec<CandleRow>)> {
    let (Some(from), Some(to)) = (fin_currency, quote_currency) else {
        return None;
    };
    if from.eq_ignore_ascii_case(to) {
        return None;
    }
    let symbol = fx_symbol(from, to);
    match Ticker::new(client, &symbol)
        .history(Some(Range::Y5), Some(Interval::D1), false)
        .await
    {
        Ok(candles) => Some((symbol, candles.iter().map(candle_row_from).collect())),
        Err(e) => {
            warnings.push(format!(
                "Câmbio {symbol} indisponível: {e} — P/VP e P/L ficam sem conversão"
            ));
            None
        }
    }
}

// ─── Dados coletados ──────────────────────────────────────────────────────────

pub struct FetchedData {
    pub display_name: Option<String>,
    pub current_price: Option<f64>,
    pub fin_currency: Option<String>,
    pub quote_currency: Option<String>,
    /// (símbolo do par, candles) quando o balanço precisa de conversão
    pub fx: Option<(String, Vec<CandleRow>)>,
    /// Ações em circulação na unidade do ticker (BDRs: em BDRs). Só buscado para BDRs.
    pub quote_shares: Option<u64>,
    /// Falhas parciais da coleta (fontes que não vieram), repassadas ao frontend
    pub warnings: Vec<String>,
    pub candles: Vec<CandleRow>,
    pub income_quarterly: Vec<IncomeRow>,
    pub income_annual: Vec<IncomeRow>,
    pub balance_quarterly: Vec<BalanceRow>,
    pub balance_annual: Vec<BalanceRow>,
    pub cashflow_quarterly: Vec<CashflowRow>,
    pub cashflow_annual: Vec<CashflowRow>,
    pub dividends: Vec<DividendRow>,
}

// ─── Fetch inicial (5 anos completos) ────────────────────────────────────────

/// Coleta todos os dados do zero (chamado na primeira adição do ativo)
pub async fn fetch_full(client: &YfClient, ticker: &str) -> Result<FetchedData> {
    let symbol = b3_symbol(ticker);
    let yf = Ticker::new(client, &symbol);

    let (
        history_res,
        q_income_res,
        a_income_res,
        q_balance_res,
        a_balance_res,
        q_cashflow_res,
        a_cashflow_res,
        dividends_res,
        fast_info_res,
        fin_currency,
    ) = tokio::join!(
        yf.history_builder()
            .range(Range::Y5)
            .interval(Interval::D1)
            .auto_adjust(false)
            .actions(true)
            .fetch(),
        yf.quarterly_income_stmt(None),
        yf.income_stmt(None),
        yf.quarterly_balance_sheet(None),
        yf.balance_sheet(None),
        yf.quarterly_cashflow(None),
        yf.cashflow(None),
        yf.dividends(Some(Range::Y5)),
        yf.fast_info(),
        fetch_statement_currency(&symbol),
    );

    let mut warnings: Vec<String> = Vec::new();

    let candles = history_res
        .with_context(|| format!("Falha ao buscar histórico de {ticker}"))?
        .iter()
        .map(candle_row_from)
        .collect();

    let income_quarterly = take(q_income_res, "DRE trimestral", &mut warnings).iter().map(income_row_from).collect();
    let income_annual = take(a_income_res, "DRE anual", &mut warnings).iter().map(income_row_from).collect();
    let balance_quarterly = take(q_balance_res, "Balanço trimestral", &mut warnings).iter().map(balance_row_from).collect();
    let balance_annual = take(a_balance_res, "Balanço anual", &mut warnings).iter().map(balance_row_from).collect();
    let cashflow_quarterly = take(q_cashflow_res, "Fluxo de caixa trimestral", &mut warnings).iter().map(cashflow_row_from).collect();
    let cashflow_annual = take(a_cashflow_res, "Fluxo de caixa anual", &mut warnings).iter().map(cashflow_row_from).collect();

    let dividends = take(dividends_res, "Dividendos", &mut warnings)
        .into_iter()
        .map(|(ts, amount)| DividendRow { ts, amount })
        .collect();

    let (display_name, current_price, quote_currency) = quote_info(fast_info_res, &mut warnings);
    if fin_currency.is_none() {
        warnings.push("Moeda do balanço não identificada — assumida igual à da cotação".into());
    }
    let fx = fetch_fx_if_needed(
        client,
        fin_currency.as_deref(),
        quote_currency.as_deref(),
        &mut warnings,
    )
    .await;
    let quote_shares = if is_bdr_ticker(ticker) {
        fetch_quote_shares(&symbol, &mut warnings).await
    } else {
        None
    };

    Ok(FetchedData {
        display_name,
        current_price,
        fin_currency,
        quote_currency,
        fx,
        quote_shares,
        warnings,
        candles,
        income_quarterly,
        income_annual,
        balance_quarterly,
        balance_annual,
        cashflow_quarterly,
        cashflow_annual,
        dividends,
    })
}

// ─── Fetch incremental (apenas dados novos desde o último registro) ───────────

/// Coleta apenas os dados faltantes desde `last_candle_ts` até agora.
/// Fundamentals (income, balance) são sempre re-coletados pois podem ser revisados.
pub async fn fetch_incremental(
    client: &YfClient,
    ticker: &str,
    last_candle_ts: Option<i64>,
) -> Result<FetchedData> {
    let symbol = b3_symbol(ticker);
    let yf = Ticker::new(client, &symbol);
    let mut warnings: Vec<String> = Vec::new();

    let new_candles: Vec<Candle> = if let Some(last_ts) = last_candle_ts {
        let start: DateTime<Utc> = Utc
            .timestamp_opt(last_ts + 86_400, 0)
            .single()
            .unwrap_or_else(Utc::now);
        let end = Utc::now();
        if start >= end {
            vec![]
        } else {
            let recent = yf
                .history_builder()
                .between(start, end)
                .interval(Interval::D1)
                .auto_adjust(false)
                .prepost(false)
                .actions(true)
                .fetch()
                .await;
            take(recent, "Cotações recentes", &mut warnings)
        }
    } else {
        yf.history_builder()
            .range(Range::Y5)
            .interval(Interval::D1)
            .auto_adjust(false)
            .actions(true)
            .fetch()
            .await
            .with_context(|| format!("Falha ao buscar histórico de {ticker}"))?
    };

    let (
        q_income_res,
        a_income_res,
        q_balance_res,
        a_balance_res,
        q_cashflow_res,
        a_cashflow_res,
        dividends_res,
        fast_info_res,
        fin_currency,
    ) = tokio::join!(
        yf.quarterly_income_stmt(None),
        yf.income_stmt(None),
        yf.quarterly_balance_sheet(None),
        yf.balance_sheet(None),
        yf.quarterly_cashflow(None),
        yf.cashflow(None),
        yf.dividends(Some(Range::Y2)),
        yf.fast_info(),
        fetch_statement_currency(&symbol),
    );

    let candles = new_candles.iter().map(candle_row_from).collect();
    let income_quarterly = take(q_income_res, "DRE trimestral", &mut warnings).iter().map(income_row_from).collect();
    let income_annual = take(a_income_res, "DRE anual", &mut warnings).iter().map(income_row_from).collect();
    let balance_quarterly = take(q_balance_res, "Balanço trimestral", &mut warnings).iter().map(balance_row_from).collect();
    let balance_annual = take(a_balance_res, "Balanço anual", &mut warnings).iter().map(balance_row_from).collect();
    let cashflow_quarterly = take(q_cashflow_res, "Fluxo de caixa trimestral", &mut warnings).iter().map(cashflow_row_from).collect();
    let cashflow_annual = take(a_cashflow_res, "Fluxo de caixa anual", &mut warnings).iter().map(cashflow_row_from).collect();

    let dividends = take(dividends_res, "Dividendos", &mut warnings)
        .into_iter()
        .map(|(ts, amount)| DividendRow { ts, amount })
        .collect();

    let (display_name, current_price, quote_currency) = quote_info(fast_info_res, &mut warnings);
    if fin_currency.is_none() {
        warnings.push("Moeda do balanço não identificada — assumida igual à da cotação".into());
    }
    let fx = fetch_fx_if_needed(
        client,
        fin_currency.as_deref(),
        quote_currency.as_deref(),
        &mut warnings,
    )
    .await;
    let quote_shares = if is_bdr_ticker(ticker) {
        fetch_quote_shares(&symbol, &mut warnings).await
    } else {
        None
    };

    Ok(FetchedData {
        display_name,
        current_price,
        fin_currency,
        quote_currency,
        fx,
        quote_shares,
        warnings,
        candles,
        income_quarterly,
        income_annual,
        balance_quarterly,
        balance_annual,
        cashflow_quarterly,
        cashflow_annual,
        dividends,
    })
}
