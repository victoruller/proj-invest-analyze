use serde::{Deserialize, Serialize};

// ─── Configurações do usuário ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    /// Taxa de retorno desejada (r). Padrão: 0.12 (12%)
    pub desired_return_rate: f64,
    /// Taxa de crescimento de fallback (g) quando CAGR não pode ser calculado. Padrão: 0.05 (5%)
    pub fallback_growth_rate: f64,
    /// Liquidez diária mínima em R$. Padrão: 200_000.0
    pub min_daily_liquidity: f64,
    /// Taxa Selic (só referência visual). Padrão: 0.1075
    pub selic_rate: f64,
    /// Teto do g calculado por CAGR (crescimento perpétuo no Gordon). Padrão: 0.06 (6%)
    pub max_growth_rate: f64,
    /// DY mínimo exigido para sinalizar COMPRA. Padrão: 0.0 (sem filtro de renda)
    pub min_dividend_yield: f64,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            desired_return_rate: 0.12,
            fallback_growth_rate: 0.05,
            min_daily_liquidity: 200_000.0,
            selic_rate: 0.1075,
            max_growth_rate: 0.06,
            min_dividend_yield: 0.0,
        }
    }
}

// ─── Watchlist ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchlistEntry {
    pub ticker: String,
    pub display_name: Option<String>,
    pub added_at: i64,
    pub last_fetched_at: Option<i64>,
}

// ─── Resumo de métricas de uma ação ──────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StockSummary {
    pub ticker: String,
    pub display_name: Option<String>,
    pub current_price: Option<f64>,

    // ── Métricas atuais (TTM / mais recente) ──
    pub liquidez_diaria: Option<f64>,    // Média 252 pregões (Close × Volume)
    pub pvp_atual: Option<f64>,          // Preço atual / VP por ação (balanço mais recente)
    pub roe_atual: Option<f64>,          // ROE TTM (últimos 4 trimestres)
    pub dy_atual: Option<f64>,           // Dividendos 12 meses / preço atual
    pub pvp_justo: Option<f64>,          // (ROE TTM - g) / (r - g)
    pub pvp_justo_5a: Option<f64>,       // (ROE médio 5a - g) / (r - g)

    // ── Médias 1 ano ──
    pub pvp_medio_1a: Option<f64>,       // Média do P/VP diário nos últimos 12 meses

    // ── Médias históricas 5 anos ──
    pub pvp_medio_5a: Option<f64>,       // Média anual do P/VP nos últimos 5 anos
    pub roe_medio_5a: Option<f64>,       // Média anual do ROE nos últimos 5 anos
    pub dy_medio_5a: Option<f64>,        // Média anual do DY nos últimos 5 anos

    // ── Extras calculados ──
    pub g_usado: Option<f64>,            // Taxa de crescimento usada no P/VP Justo (CAGR ou fallback)
    pub g_fonte: Option<String>,         // "cagr" ou "fallback"
    pub vp_por_acao: Option<f64>,        // VP/Ação = equity / shares (balanço mais recente)
    pub net_income_ttm: Option<f64>,     // Lucro líquido TTM (soma 4 trimestres) em R$
    pub dividends_ttm: Option<f64>,      // Dividendos por ação pagos nos últimos 12 meses em R$

    // ── Novas métricas financeiras ──
    pub receita_ttm: Option<f64>,        // Receita TTM (soma 4 trimestres) em R$
    pub margem_liquida: Option<f64>,     // Net margin = net_income_ttm / receita_ttm
    pub margem_operacional: Option<f64>, // EBIT margin = operating_income_ttm / receita_ttm
    pub roic: Option<f64>,               // NOPAT / invested_capital
    pub net_debt: Option<f64>,           // Dívida líquida = long_term_debt - cash
    pub divida_liquida_pl: Option<f64>,  // net_debt / total_equity
    pub divida_liquida_ebit: Option<f64>,// net_debt / operating_income_ttm
    pub pl: Option<f64>,                 // P/E = price / EPS
    pub ev_ebit: Option<f64>,            // EV/EBIT
    pub flags: Vec<String>,              // Flags de alerta/oportunidade
    pub warnings: Vec<String>,           // Falhas parciais da última coleta (não persistido)

    // ── Situações ──
    pub situacao_atual: Situacao,
    pub situacao_atual_reason: String,
    pub situacao_5a: Situacao,
    pub situacao_5a_reason: String,

    pub last_fetched_at: Option<i64>,

    /// JSON com detalhes de cada cálculo (para tooltips)
    pub details: MetricDetails,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Situacao {
    Compra,
    Manter,
    Venda,
    DadosInsuficientes,
}

impl std::fmt::Display for Situacao {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Situacao::Compra => write!(f, "COMPRA"),
            Situacao::Manter => write!(f, "MANTER"),
            Situacao::Venda => write!(f, "VENDA"),
            Situacao::DadosInsuficientes => write!(f, "DADOS_INSUFICIENTES"),
        }
    }
}

impl std::str::FromStr for Situacao {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "COMPRA" => Ok(Situacao::Compra),
            "MANTER" => Ok(Situacao::Manter),
            "VENDA" => Ok(Situacao::Venda),
            _ => Ok(Situacao::DadosInsuficientes),
        }
    }
}

/// JSON com detalhes de cada cálculo — exibidos como tooltip no frontend
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MetricDetails {
    pub liquidez: Option<String>,
    pub pvp_atual: Option<String>,
    pub roe_atual: Option<String>,
    pub dy_atual: Option<String>,
    pub pvp_justo: Option<String>,
    pub pvp_justo_5a: Option<String>,
    pub pvp_medio_1a: Option<String>,
    pub pvp_medio_5a: Option<String>,
    pub roe_medio_5a: Option<String>,
    pub dy_medio_5a: Option<String>,
    pub g_usado: Option<String>,
    // Novas métricas
    pub receita_ttm: Option<String>,
    pub margem_liquida: Option<String>,
    pub margem_operacional: Option<String>,
    pub roic: Option<String>,
    pub net_debt: Option<String>,
    pub divida_liquida_pl: Option<String>,
    pub divida_liquida_ebit: Option<String>,
    pub pl: Option<String>,
    pub ev_ebit: Option<String>,
}

// ─── Tipos internos (não serializados ao frontend) ───────────────────────────

pub struct CandleRow {
    pub ts: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: Option<u64>,
}

pub struct IncomeRow {
    pub period_label: String,
    pub period_ts: Option<i64>,
    pub net_income: Option<f64>,
    pub total_revenue: Option<f64>,
    pub operating_income: Option<f64>,
}

pub struct BalanceRow {
    pub period_label: String,
    pub period_ts: Option<i64>,
    pub total_equity: Option<f64>,
    pub shares_outstanding: Option<u64>,
    pub cash: Option<f64>,
    pub long_term_debt: Option<f64>,
    pub total_assets: Option<f64>,
    pub total_liabilities: Option<f64>,
}

pub struct CashflowRow {
    pub period_label: String,
    pub period_ts: Option<i64>,
    pub operating_cashflow: Option<f64>,
    pub free_cash_flow: Option<f64>,
}

pub struct DividendRow {
    pub ts: i64,
    pub amount: f64,
}
