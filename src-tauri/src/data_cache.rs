use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::PathBuf;
use std::sync::Mutex;

use crate::models::{
    AppSettings, BalanceRow, CandleRow, CashflowRow, DividendRow, IncomeRow, MetricDetails,
    Situacao, StockSummary, WatchlistEntry,
};

// ─── Schema ──────────────────────────────────────────────────────────────────

const SCHEMA_SQL: &str = "
PRAGMA journal_mode=WAL;

CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS watchlist (
    ticker          TEXT PRIMARY KEY,
    display_name    TEXT,
    added_at        INTEGER NOT NULL,
    last_fetched_at INTEGER,
    fin_currency    TEXT,
    quote_currency  TEXT,
    price_basis     TEXT,
    quote_shares    INTEGER
);

CREATE TABLE IF NOT EXISTS price_history (
    ticker  TEXT    NOT NULL,
    ts      INTEGER NOT NULL,
    open    REAL    NOT NULL,
    high    REAL    NOT NULL,
    low     REAL    NOT NULL,
    close   REAL    NOT NULL,
    volume  INTEGER,
    PRIMARY KEY (ticker, ts)
);

CREATE TABLE IF NOT EXISTS income_quarterly (
    ticker           TEXT    NOT NULL,
    period_label     TEXT    NOT NULL,
    period_ts        INTEGER,
    net_income       REAL,
    total_revenue    REAL,
    operating_income REAL,
    PRIMARY KEY (ticker, period_label)
);

CREATE TABLE IF NOT EXISTS income_annual (
    ticker           TEXT    NOT NULL,
    period_label     TEXT    NOT NULL,
    period_ts        INTEGER,
    net_income       REAL,
    total_revenue    REAL,
    operating_income REAL,
    PRIMARY KEY (ticker, period_label)
);

CREATE TABLE IF NOT EXISTS balance_quarterly (
    ticker              TEXT    NOT NULL,
    period_label        TEXT    NOT NULL,
    period_ts           INTEGER,
    total_equity        REAL,
    shares_outstanding  INTEGER,
    cash                REAL,
    long_term_debt      REAL,
    total_assets        REAL,
    total_liabilities   REAL,
    PRIMARY KEY (ticker, period_label)
);

CREATE TABLE IF NOT EXISTS balance_annual (
    ticker              TEXT    NOT NULL,
    period_label        TEXT    NOT NULL,
    period_ts           INTEGER,
    total_equity        REAL,
    shares_outstanding  INTEGER,
    cash                REAL,
    long_term_debt      REAL,
    total_assets        REAL,
    total_liabilities   REAL,
    PRIMARY KEY (ticker, period_label)
);

CREATE TABLE IF NOT EXISTS cashflow_quarterly (
    ticker              TEXT    NOT NULL,
    period_label        TEXT    NOT NULL,
    period_ts           INTEGER,
    operating_cashflow  REAL,
    free_cash_flow      REAL,
    PRIMARY KEY (ticker, period_label)
);

CREATE TABLE IF NOT EXISTS cashflow_annual (
    ticker              TEXT    NOT NULL,
    period_label        TEXT    NOT NULL,
    period_ts           INTEGER,
    operating_cashflow  REAL,
    free_cash_flow      REAL,
    PRIMARY KEY (ticker, period_label)
);

CREATE TABLE IF NOT EXISTS dividends (
    ticker  TEXT    NOT NULL,
    ts      INTEGER NOT NULL,
    amount  REAL    NOT NULL,
    PRIMARY KEY (ticker, ts)
);

CREATE TABLE IF NOT EXISTS metrics_cache (
    ticker              TEXT PRIMARY KEY,
    current_price       REAL,
    liquidez_diaria     REAL,
    pvp_atual           REAL,
    roe_atual           REAL,
    dy_atual            REAL,
    pvp_justo           REAL,
    pvp_medio_1a        REAL,
    pvp_medio_5a        REAL,
    roe_medio_5a        REAL,
    dy_medio_5a         REAL,
    situacao_atual      TEXT,
    situacao_atual_reason TEXT,
    situacao_5a         TEXT,
    situacao_5a_reason  TEXT,
    liquidez_detail     TEXT,
    pvp_atual_detail    TEXT,
    roe_atual_detail    TEXT,
    dy_atual_detail     TEXT,
    pvp_justo_detail    TEXT,
    pvp_medio_1a_detail TEXT,
    pvp_medio_5a_detail TEXT,
    roe_medio_5a_detail TEXT,
    dy_medio_5a_detail  TEXT,
    g_usado_detail      TEXT,
    g_usado             REAL,
    g_fonte             TEXT,
    vp_por_acao         REAL,
    net_income_ttm      REAL,
    dividends_ttm       REAL,
    margem_liquida      REAL,
    margem_operacional  REAL,
    roic                REAL,
    net_debt            REAL,
    divida_liquida_pl   REAL,
    divida_liquida_ebit REAL,
    pl                  REAL,
    ev_ebit             REAL,
    receita_ttm         REAL,
    flags               TEXT,
    margem_liquida_detail      TEXT,
    margem_operacional_detail  TEXT,
    roic_detail                TEXT,
    net_debt_detail            TEXT,
    divida_liquida_pl_detail   TEXT,
    divida_liquida_ebit_detail TEXT,
    pl_detail                  TEXT,
    ev_ebit_detail             TEXT,
    receita_ttm_detail         TEXT,
    pvp_justo_5a               REAL,
    pvp_justo_5a_detail        TEXT,
    computed_at         INTEGER
);
";

// ─── DataCache ───────────────────────────────────────────────────────────────

pub struct DataCache {
    conn: Mutex<Connection>,
}

impl DataCache {
    /// Abre (ou cria) o banco em {project_root}/finance_data.db
    pub fn open() -> Result<Self> {
        let path = db_path();
        let conn = Connection::open(&path)
            .with_context(|| format!("Falha ao abrir banco em {}", path.display()))?;
        conn.execute_batch(SCHEMA_SQL).context("Falha ao criar schema")?;
        // Migração: adiciona colunas novas em bancos já existentes (ignora erro se já existir)
        for col in &[
            "ALTER TABLE metrics_cache ADD COLUMN g_usado_detail TEXT",
            "ALTER TABLE metrics_cache ADD COLUMN g_usado REAL",
            "ALTER TABLE metrics_cache ADD COLUMN g_fonte TEXT",
            "ALTER TABLE metrics_cache ADD COLUMN vp_por_acao REAL",
            "ALTER TABLE metrics_cache ADD COLUMN net_income_ttm REAL",
            "ALTER TABLE metrics_cache ADD COLUMN dividends_ttm REAL",
            // New metrics columns
            "ALTER TABLE metrics_cache ADD COLUMN margem_liquida REAL",
            "ALTER TABLE metrics_cache ADD COLUMN margem_operacional REAL",
            "ALTER TABLE metrics_cache ADD COLUMN roic REAL",
            "ALTER TABLE metrics_cache ADD COLUMN net_debt REAL",
            "ALTER TABLE metrics_cache ADD COLUMN divida_liquida_pl REAL",
            "ALTER TABLE metrics_cache ADD COLUMN divida_liquida_ebit REAL",
            "ALTER TABLE metrics_cache ADD COLUMN pl REAL",
            "ALTER TABLE metrics_cache ADD COLUMN ev_ebit REAL",
            "ALTER TABLE metrics_cache ADD COLUMN receita_ttm REAL",
            "ALTER TABLE metrics_cache ADD COLUMN flags TEXT",
            "ALTER TABLE metrics_cache ADD COLUMN margem_liquida_detail TEXT",
            "ALTER TABLE metrics_cache ADD COLUMN margem_operacional_detail TEXT",
            "ALTER TABLE metrics_cache ADD COLUMN roic_detail TEXT",
            "ALTER TABLE metrics_cache ADD COLUMN net_debt_detail TEXT",
            "ALTER TABLE metrics_cache ADD COLUMN divida_liquida_pl_detail TEXT",
            "ALTER TABLE metrics_cache ADD COLUMN divida_liquida_ebit_detail TEXT",
            "ALTER TABLE metrics_cache ADD COLUMN pl_detail TEXT",
            "ALTER TABLE metrics_cache ADD COLUMN ev_ebit_detail TEXT",
            "ALTER TABLE metrics_cache ADD COLUMN receita_ttm_detail TEXT",
            "ALTER TABLE metrics_cache ADD COLUMN pvp_justo_5a REAL",
            "ALTER TABLE metrics_cache ADD COLUMN pvp_justo_5a_detail TEXT",
            "ALTER TABLE watchlist ADD COLUMN fin_currency TEXT",
            "ALTER TABLE watchlist ADD COLUMN quote_currency TEXT",
            "ALTER TABLE watchlist ADD COLUMN price_basis TEXT",
            "ALTER TABLE watchlist ADD COLUMN quote_shares INTEGER",
            // New income columns
            "ALTER TABLE income_quarterly ADD COLUMN total_revenue REAL",
            "ALTER TABLE income_quarterly ADD COLUMN operating_income REAL",
            "ALTER TABLE income_annual ADD COLUMN total_revenue REAL",
            "ALTER TABLE income_annual ADD COLUMN operating_income REAL",
            // New balance columns
            "ALTER TABLE balance_quarterly ADD COLUMN cash REAL",
            "ALTER TABLE balance_quarterly ADD COLUMN long_term_debt REAL",
            "ALTER TABLE balance_quarterly ADD COLUMN total_assets REAL",
            "ALTER TABLE balance_quarterly ADD COLUMN total_liabilities REAL",
            "ALTER TABLE balance_annual ADD COLUMN cash REAL",
            "ALTER TABLE balance_annual ADD COLUMN long_term_debt REAL",
            "ALTER TABLE balance_annual ADD COLUMN total_assets REAL",
            "ALTER TABLE balance_annual ADD COLUMN total_liabilities REAL",
            // New cashflow tables (CREATE IF NOT EXISTS handles new DBs; for existing ones we may need explicit create)
            "CREATE TABLE IF NOT EXISTS cashflow_quarterly (ticker TEXT NOT NULL, period_label TEXT NOT NULL, period_ts INTEGER, operating_cashflow REAL, free_cash_flow REAL, PRIMARY KEY (ticker, period_label))",
            "CREATE TABLE IF NOT EXISTS cashflow_annual (ticker TEXT NOT NULL, period_label TEXT NOT NULL, period_ts INTEGER, operating_cashflow REAL, free_cash_flow REAL, PRIMARY KEY (ticker, period_label))",
        ] {
            let _ = conn.execute(col, []);
        }
        Ok(Self { conn: Mutex::new(conn) })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().expect("mutex envenenado")
    }

    // ── Watchlist ────────────────────────────────────────────────────────────

    pub fn insert_watchlist(&self, ticker: &str, display_name: Option<&str>) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        self.lock().execute(
            "INSERT OR IGNORE INTO watchlist (ticker, display_name, added_at) VALUES (?1, ?2, ?3)",
            params![ticker, display_name, now],
        )?;
        Ok(())
    }

    pub fn update_display_name(&self, ticker: &str, display_name: &str) -> Result<()> {
        self.lock().execute(
            "UPDATE watchlist SET display_name = ?1 WHERE ticker = ?2",
            params![display_name, ticker],
        )?;
        Ok(())
    }

    pub fn remove_watchlist(&self, ticker: &str) -> Result<()> {
        let conn = self.lock();
        conn.execute("DELETE FROM watchlist WHERE ticker = ?1", params![ticker])?;
        conn.execute("DELETE FROM price_history WHERE ticker = ?1", params![ticker])?;
        conn.execute("DELETE FROM income_quarterly WHERE ticker = ?1", params![ticker])?;
        conn.execute("DELETE FROM income_annual WHERE ticker = ?1", params![ticker])?;
        conn.execute("DELETE FROM balance_quarterly WHERE ticker = ?1", params![ticker])?;
        conn.execute("DELETE FROM balance_annual WHERE ticker = ?1", params![ticker])?;
        conn.execute("DELETE FROM cashflow_quarterly WHERE ticker = ?1", params![ticker])?;
        conn.execute("DELETE FROM cashflow_annual WHERE ticker = ?1", params![ticker])?;
        conn.execute("DELETE FROM dividends WHERE ticker = ?1", params![ticker])?;
        conn.execute("DELETE FROM metrics_cache WHERE ticker = ?1", params![ticker])?;
        Ok(())
    }

    pub fn get_watchlist(&self) -> Result<Vec<WatchlistEntry>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT ticker, display_name, added_at, last_fetched_at FROM watchlist ORDER BY added_at",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(WatchlistEntry {
                ticker: row.get(0)?,
                display_name: row.get(1)?,
                added_at: row.get(2)?,
                last_fetched_at: row.get(3)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    pub fn update_last_fetched(&self, ticker: &str) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        self.lock().execute(
            "UPDATE watchlist SET last_fetched_at = ?1 WHERE ticker = ?2",
            params![now, ticker],
        )?;
        Ok(())
    }

    /// Atualiza moedas do balanço e da cotação (mantém o valor anterior quando vier None)
    pub fn update_currencies(
        &self,
        ticker: &str,
        fin_currency: Option<&str>,
        quote_currency: Option<&str>,
    ) -> Result<()> {
        self.lock().execute(
            "UPDATE watchlist
             SET fin_currency = COALESCE(?1, fin_currency),
                 quote_currency = COALESCE(?2, quote_currency)
             WHERE ticker = ?3",
            params![fin_currency, quote_currency, ticker],
        )?;
        Ok(())
    }

    /// (moeda do balanço, moeda da cotação)
    pub fn get_currencies(&self, ticker: &str) -> Result<(Option<String>, Option<String>)> {
        let row = self
            .lock()
            .query_row(
                "SELECT fin_currency, quote_currency FROM watchlist WHERE ticker = ?1",
                params![ticker],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(row.unwrap_or((None, None)))
    }

    /// Ações em circulação na unidade do ticker cotado (BDRs: em BDRs)
    pub fn get_quote_shares(&self, ticker: &str) -> Result<Option<u64>> {
        let shares = self
            .lock()
            .query_row(
                "SELECT quote_shares FROM watchlist WHERE ticker = ?1",
                params![ticker],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()?;
        Ok(shares.flatten().map(|s| s as u64))
    }

    /// Mantém o valor anterior quando a coleta não trouxe o dado
    pub fn update_quote_shares(&self, ticker: &str, shares: Option<u64>) -> Result<()> {
        self.lock().execute(
            "UPDATE watchlist SET quote_shares = COALESCE(?1, quote_shares) WHERE ticker = ?2",
            params![shares.map(|s| s as i64), ticker],
        )?;
        Ok(())
    }

    /// Base de ajuste dos preços já salvos para o ticker (ver `PRICE_BASIS` em lib.rs)
    pub fn get_price_basis(&self, ticker: &str) -> Result<Option<String>> {
        let basis = self
            .lock()
            .query_row(
                "SELECT price_basis FROM watchlist WHERE ticker = ?1",
                params![ticker],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?;
        Ok(basis.flatten())
    }

    pub fn update_price_basis(&self, ticker: &str, basis: &str) -> Result<()> {
        self.lock().execute(
            "UPDATE watchlist SET price_basis = ?1 WHERE ticker = ?2",
            params![basis, ticker],
        )?;
        Ok(())
    }

    // ── Price history ────────────────────────────────────────────────────────

    pub fn last_price_ts(&self, ticker: &str) -> Result<Option<i64>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT MAX(ts) FROM price_history WHERE ticker = ?1",
        )?;
        let ts: Option<i64> = stmt.query_row(params![ticker], |r| r.get(0))?;
        Ok(ts)
    }

    /// Remove pregões anteriores a `ts` — usado ao trocar a base de ajuste dos preços,
    /// para não deixar um trecho antigo fora da janela re-coletada em outra base.
    pub fn delete_candles_before(&self, ticker: &str, ts: i64) -> Result<usize> {
        let removed = self.lock().execute(
            "DELETE FROM price_history WHERE ticker = ?1 AND ts < ?2",
            params![ticker, ts],
        )?;
        Ok(removed)
    }

    pub fn upsert_candles(&self, ticker: &str, candles: &[CandleRow]) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO price_history (ticker, ts, open, high, low, close, volume)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for c in candles {
                stmt.execute(params![ticker, c.ts, c.open, c.high, c.low, c.close, c.volume])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Retorna os últimos `n` candles ordenados do mais antigo ao mais recente
    pub fn get_last_n_candles(&self, ticker: &str, n: usize) -> Result<Vec<CandleRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT ts, open, high, low, close, volume FROM (
               SELECT ts, open, high, low, close, volume FROM price_history
               WHERE ticker = ?1 ORDER BY ts DESC LIMIT ?2
             ) ORDER BY ts ASC",
        )?;
        let rows = stmt.query_map(params![ticker, n as i64], |row| {
            Ok(CandleRow {
                ts: row.get(0)?,
                open: row.get(1)?,
                high: row.get(2)?,
                low: row.get(3)?,
                close: row.get(4)?,
                volume: row.get(5)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    /// Retorna candles de um período (timestamps Unix, inclusive)
    pub fn get_candles_between(&self, ticker: &str, from_ts: i64, to_ts: i64) -> Result<Vec<CandleRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT ts, open, high, low, close, volume FROM price_history
             WHERE ticker = ?1 AND ts >= ?2 AND ts <= ?3 ORDER BY ts ASC",
        )?;
        let rows = stmt.query_map(params![ticker, from_ts, to_ts], |row| {
            Ok(CandleRow {
                ts: row.get(0)?,
                open: row.get(1)?,
                high: row.get(2)?,
                low: row.get(3)?,
                close: row.get(4)?,
                volume: row.get(5)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    // ── Income statement ─────────────────────────────────────────────────────

    pub fn upsert_income_quarterly(&self, ticker: &str, rows: &[IncomeRow]) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO income_quarterly
                 (ticker, period_label, period_ts, net_income, total_revenue, operating_income)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for r in rows {
                stmt.execute(params![
                    ticker, r.period_label, r.period_ts, r.net_income,
                    r.total_revenue, r.operating_income
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn upsert_income_annual(&self, ticker: &str, rows: &[IncomeRow]) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO income_annual
                 (ticker, period_label, period_ts, net_income, total_revenue, operating_income)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for r in rows {
                stmt.execute(params![
                    ticker, r.period_label, r.period_ts, r.net_income,
                    r.total_revenue, r.operating_income
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Últimos `n` trimestres (campos podem ser nulos) ordenados do mais antigo ao mais recente
    pub fn get_quarterly_income(&self, ticker: &str, n: usize) -> Result<Vec<IncomeRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT period_label, period_ts, net_income, total_revenue, operating_income FROM (
               SELECT period_label, period_ts, net_income, total_revenue, operating_income
               FROM income_quarterly
               WHERE ticker = ?1
               ORDER BY COALESCE(period_ts, 0) DESC, period_label DESC LIMIT ?2
             ) ORDER BY COALESCE(period_ts, 0) ASC, period_label ASC",
        )?;
        let rows = stmt.query_map(params![ticker, n as i64], |row| {
            Ok(IncomeRow {
                period_label: row.get(0)?,
                period_ts: row.get(1)?,
                net_income: row.get(2)?,
                total_revenue: row.get(3)?,
                operating_income: row.get(4)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    /// Últimos `n` anos com net_income ordenados do mais antigo ao mais recente
    pub fn get_annual_income(&self, ticker: &str, n: usize) -> Result<Vec<IncomeRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT period_label, period_ts, net_income, total_revenue, operating_income FROM (
               SELECT period_label, period_ts, net_income, total_revenue, operating_income
               FROM income_annual
               WHERE ticker = ?1 AND net_income IS NOT NULL
               ORDER BY COALESCE(period_ts, 0) DESC, period_label DESC LIMIT ?2
             ) ORDER BY COALESCE(period_ts, 0) ASC, period_label ASC",
        )?;
        let rows = stmt.query_map(params![ticker, n as i64], |row| {
            Ok(IncomeRow {
                period_label: row.get(0)?,
                period_ts: row.get(1)?,
                net_income: row.get(2)?,
                total_revenue: row.get(3)?,
                operating_income: row.get(4)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    // ── Balance sheet ────────────────────────────────────────────────────────

    pub fn upsert_balance_quarterly(&self, ticker: &str, rows: &[BalanceRow]) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO balance_quarterly
                 (ticker, period_label, period_ts, total_equity, shares_outstanding,
                  cash, long_term_debt, total_assets, total_liabilities)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for r in rows {
                stmt.execute(params![
                    ticker, r.period_label, r.period_ts, r.total_equity,
                    r.shares_outstanding.map(|s| s as i64),
                    r.cash, r.long_term_debt, r.total_assets, r.total_liabilities
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn upsert_balance_annual(&self, ticker: &str, rows: &[BalanceRow]) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO balance_annual
                 (ticker, period_label, period_ts, total_equity, shares_outstanding,
                  cash, long_term_debt, total_assets, total_liabilities)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for r in rows {
                stmt.execute(params![
                    ticker, r.period_label, r.period_ts, r.total_equity,
                    r.shares_outstanding.map(|s| s as i64),
                    r.cash, r.long_term_debt, r.total_assets, r.total_liabilities
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Balanço trimestral mais recente com equity preenchida
    pub fn get_latest_balance_quarterly(&self, ticker: &str) -> Result<Option<BalanceRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT period_label, period_ts, total_equity, shares_outstanding,
                    cash, long_term_debt, total_assets, total_liabilities
             FROM balance_quarterly
             WHERE ticker = ?1 AND total_equity IS NOT NULL
             ORDER BY COALESCE(period_ts, 0) DESC, period_label DESC LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![ticker], |row| {
            let shares: Option<i64> = row.get(3)?;
            Ok(BalanceRow {
                period_label: row.get(0)?,
                period_ts: row.get(1)?,
                total_equity: row.get(2)?,
                shares_outstanding: shares.map(|s| s as u64),
                cash: row.get(4)?,
                long_term_debt: row.get(5)?,
                total_assets: row.get(6)?,
                total_liabilities: row.get(7)?,
            })
        })?;
        Ok(rows.next().transpose()?)
    }

    /// Últimos `n` trimestres de balance com equity ordenados do mais antigo ao mais recente
    pub fn get_quarterly_balance(&self, ticker: &str, n: usize) -> Result<Vec<BalanceRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT period_label, period_ts, total_equity, shares_outstanding,
                    cash, long_term_debt, total_assets, total_liabilities FROM (
               SELECT period_label, period_ts, total_equity, shares_outstanding,
                      cash, long_term_debt, total_assets, total_liabilities
               FROM balance_quarterly
               WHERE ticker = ?1 AND total_equity IS NOT NULL
               ORDER BY COALESCE(period_ts, 0) DESC, period_label DESC LIMIT ?2
             ) ORDER BY COALESCE(period_ts, 0) ASC, period_label ASC",
        )?;
        let rows = stmt.query_map(params![ticker, n as i64], |row| {
            let shares: Option<i64> = row.get(3)?;
            Ok(BalanceRow {
                period_label: row.get(0)?,
                period_ts: row.get(1)?,
                total_equity: row.get(2)?,
                shares_outstanding: shares.map(|s| s as u64),
                cash: row.get(4)?,
                long_term_debt: row.get(5)?,
                total_assets: row.get(6)?,
                total_liabilities: row.get(7)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    /// Últimos `n` anos de balance ordenados do mais antigo ao mais recente
    pub fn get_annual_balance(&self, ticker: &str, n: usize) -> Result<Vec<BalanceRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT period_label, period_ts, total_equity, shares_outstanding,
                    cash, long_term_debt, total_assets, total_liabilities FROM (
               SELECT period_label, period_ts, total_equity, shares_outstanding,
                      cash, long_term_debt, total_assets, total_liabilities
               FROM balance_annual
               WHERE ticker = ?1 AND total_equity IS NOT NULL
               ORDER BY COALESCE(period_ts, 0) DESC, period_label DESC LIMIT ?2
             ) ORDER BY COALESCE(period_ts, 0) ASC, period_label ASC",
        )?;
        let rows = stmt.query_map(params![ticker, n as i64], |row| {
            let shares: Option<i64> = row.get(3)?;
            Ok(BalanceRow {
                period_label: row.get(0)?,
                period_ts: row.get(1)?,
                total_equity: row.get(2)?,
                shares_outstanding: shares.map(|s| s as u64),
                cash: row.get(4)?,
                long_term_debt: row.get(5)?,
                total_assets: row.get(6)?,
                total_liabilities: row.get(7)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    // ── Cashflow ─────────────────────────────────────────────────────────────

    pub fn upsert_cashflow(&self, ticker: &str, rows: &[CashflowRow], quarterly: bool) -> Result<()> {
        let table = if quarterly { "cashflow_quarterly" } else { "cashflow_annual" };
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        {
            let sql = format!(
                "INSERT OR REPLACE INTO {} (ticker, period_label, period_ts, operating_cashflow, free_cash_flow)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                table
            );
            let mut stmt = tx.prepare(&sql)?;
            for r in rows {
                stmt.execute(params![
                    ticker, r.period_label, r.period_ts,
                    r.operating_cashflow, r.free_cash_flow
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Últimos `n` trimestres de cashflow ordenados do mais antigo ao mais recente
    pub fn get_quarterly_cashflow(&self, ticker: &str, n: usize) -> Result<Vec<CashflowRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT period_label, period_ts, operating_cashflow, free_cash_flow FROM (
               SELECT period_label, period_ts, operating_cashflow, free_cash_flow
               FROM cashflow_quarterly
               WHERE ticker = ?1
               ORDER BY COALESCE(period_ts, 0) DESC, period_label DESC LIMIT ?2
             ) ORDER BY COALESCE(period_ts, 0) ASC, period_label ASC",
        )?;
        let rows = stmt.query_map(params![ticker, n as i64], |row| {
            Ok(CashflowRow {
                period_label: row.get(0)?,
                period_ts: row.get(1)?,
                operating_cashflow: row.get(2)?,
                free_cash_flow: row.get(3)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    // ── Dividendos ───────────────────────────────────────────────────────────

    pub fn upsert_dividends(&self, ticker: &str, rows: &[DividendRow]) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR REPLACE INTO dividends (ticker, ts, amount) VALUES (?1, ?2, ?3)",
            )?;
            for d in rows {
                stmt.execute(params![ticker, d.ts, d.amount])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Dividendos dos últimos 12 meses (trailing twelve months)
    pub fn get_ttm_dividends(&self, ticker: &str) -> Result<Vec<DividendRow>> {
        let since = chrono::Utc::now().timestamp() - 365 * 24 * 3600;
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT ts, amount FROM dividends WHERE ticker = ?1 AND ts >= ?2 ORDER BY ts ASC",
        )?;
        let rows = stmt.query_map(params![ticker, since], |row| {
            Ok(DividendRow { ts: row.get(0)?, amount: row.get(1)? })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    /// Todos os dividendos de um ticker, ordenados por data
    pub fn get_all_dividends(&self, ticker: &str) -> Result<Vec<DividendRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT ts, amount FROM dividends WHERE ticker = ?1 ORDER BY ts ASC",
        )?;
        let rows = stmt.query_map(params![ticker], |row| {
            Ok(DividendRow { ts: row.get(0)?, amount: row.get(1)? })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    // ── Metrics cache ────────────────────────────────────────────────────────

    pub fn upsert_metrics(&self, s: &StockSummary) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        let flags_json = serde_json::to_string(&s.flags).unwrap_or_else(|_| "[]".to_string());
        self.lock().execute(
            "INSERT OR REPLACE INTO metrics_cache (
               ticker, current_price, liquidez_diaria, pvp_atual, roe_atual, dy_atual, pvp_justo,
               pvp_medio_1a, pvp_medio_5a, roe_medio_5a, dy_medio_5a,
               situacao_atual, situacao_atual_reason, situacao_5a, situacao_5a_reason,
               liquidez_detail, pvp_atual_detail, roe_atual_detail, dy_atual_detail,
               pvp_justo_detail, pvp_medio_1a_detail, pvp_medio_5a_detail,
               roe_medio_5a_detail, dy_medio_5a_detail, g_usado_detail,
               g_usado, g_fonte, vp_por_acao, net_income_ttm, dividends_ttm,
               margem_liquida, margem_operacional, roic, net_debt,
               divida_liquida_pl, divida_liquida_ebit, pl, ev_ebit, receita_ttm, flags,
               margem_liquida_detail, margem_operacional_detail, roic_detail, net_debt_detail,
               divida_liquida_pl_detail, divida_liquida_ebit_detail, pl_detail, ev_ebit_detail,
               receita_ttm_detail, pvp_justo_5a, pvp_justo_5a_detail,
               computed_at
             ) VALUES (
               ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,
               ?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,
               ?26,?27,?28,?29,?30,
               ?31,?32,?33,?34,?35,?36,?37,?38,?39,?40,
               ?41,?42,?43,?44,?45,?46,?47,?48,?49,
               ?50,?51,
               ?52
             )",
            params![
                s.ticker,
                s.current_price,
                s.liquidez_diaria,
                s.pvp_atual,
                s.roe_atual,
                s.dy_atual,
                s.pvp_justo,
                s.pvp_medio_1a,
                s.pvp_medio_5a,
                s.roe_medio_5a,
                s.dy_medio_5a,
                s.situacao_atual.to_string(),
                s.situacao_atual_reason,
                s.situacao_5a.to_string(),
                s.situacao_5a_reason,
                s.details.liquidez,
                s.details.pvp_atual,
                s.details.roe_atual,
                s.details.dy_atual,
                s.details.pvp_justo,
                s.details.pvp_medio_1a,
                s.details.pvp_medio_5a,
                s.details.roe_medio_5a,
                s.details.dy_medio_5a,
                s.details.g_usado,
                s.g_usado,
                s.g_fonte,
                s.vp_por_acao,
                s.net_income_ttm,
                s.dividends_ttm,
                s.margem_liquida,
                s.margem_operacional,
                s.roic,
                s.net_debt,
                s.divida_liquida_pl,
                s.divida_liquida_ebit,
                s.pl,
                s.ev_ebit,
                s.receita_ttm,
                flags_json,
                s.details.margem_liquida,
                s.details.margem_operacional,
                s.details.roic,
                s.details.net_debt,
                s.details.divida_liquida_pl,
                s.details.divida_liquida_ebit,
                s.details.pl,
                s.details.ev_ebit,
                s.details.receita_ttm,
                s.pvp_justo_5a,
                s.details.pvp_justo_5a,
                now,
            ],
        )?;
        Ok(())
    }

    pub fn get_all_summaries(&self) -> Result<Vec<StockSummary>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT w.ticker, w.display_name, w.last_fetched_at,
                    m.current_price, m.liquidez_diaria, m.pvp_atual, m.roe_atual, m.dy_atual,
                    m.pvp_justo, m.pvp_medio_1a, m.pvp_medio_5a, m.roe_medio_5a, m.dy_medio_5a,
                    m.situacao_atual, m.situacao_atual_reason, m.situacao_5a, m.situacao_5a_reason,
                    m.liquidez_detail, m.pvp_atual_detail, m.roe_atual_detail, m.dy_atual_detail,
                    m.pvp_justo_detail, m.pvp_medio_1a_detail, m.pvp_medio_5a_detail,
                    m.roe_medio_5a_detail, m.dy_medio_5a_detail, m.g_usado_detail,
                    m.g_usado, m.g_fonte, m.vp_por_acao, m.net_income_ttm, m.dividends_ttm,
                    m.margem_liquida, m.margem_operacional, m.roic, m.net_debt,
                    m.divida_liquida_pl, m.divida_liquida_ebit, m.pl, m.ev_ebit, m.receita_ttm,
                    m.flags,
                    m.margem_liquida_detail, m.margem_operacional_detail, m.roic_detail,
                    m.net_debt_detail, m.divida_liquida_pl_detail, m.divida_liquida_ebit_detail,
                    m.pl_detail, m.ev_ebit_detail, m.receita_ttm_detail,
                    m.pvp_justo_5a, m.pvp_justo_5a_detail
             FROM watchlist w
             LEFT JOIN metrics_cache m ON w.ticker = m.ticker
             ORDER BY w.added_at",
        )?;
        let rows = stmt.query_map([], |row| {
            let sit_a: Option<String> = row.get(13)?;
            let sit_5: Option<String> = row.get(15)?;
            let flags_json: Option<String> = row.get(41)?;
            let flags: Vec<String> = flags_json
                .and_then(|j| serde_json::from_str(&j).ok())
                .unwrap_or_default();
            Ok(StockSummary {
                ticker: row.get(0)?,
                display_name: row.get(1)?,
                last_fetched_at: row.get(2)?,
                current_price: row.get(3)?,
                liquidez_diaria: row.get(4)?,
                pvp_atual: row.get(5)?,
                roe_atual: row.get(6)?,
                dy_atual: row.get(7)?,
                pvp_justo: row.get(8)?,
                pvp_justo_5a: row.get(51)?,
                pvp_medio_1a: row.get(9)?,
                pvp_medio_5a: row.get(10)?,
                roe_medio_5a: row.get(11)?,
                dy_medio_5a: row.get(12)?,
                situacao_atual: sit_a
                    .as_deref()
                    .map(|s| s.parse().unwrap_or(Situacao::DadosInsuficientes))
                    .unwrap_or(Situacao::DadosInsuficientes),
                situacao_atual_reason: row.get(14).unwrap_or_default(),
                situacao_5a: sit_5
                    .as_deref()
                    .map(|s| s.parse().unwrap_or(Situacao::DadosInsuficientes))
                    .unwrap_or(Situacao::DadosInsuficientes),
                situacao_5a_reason: row.get(16).unwrap_or_default(),
                details: MetricDetails {
                    liquidez: row.get(17)?,
                    pvp_atual: row.get(18)?,
                    roe_atual: row.get(19)?,
                    dy_atual: row.get(20)?,
                    pvp_justo: row.get(21)?,
                    pvp_medio_1a: row.get(22)?,
                    pvp_medio_5a: row.get(23)?,
                    roe_medio_5a: row.get(24)?,
                    dy_medio_5a: row.get(25)?,
                    g_usado: row.get(26)?,
                    margem_liquida: row.get(42)?,
                    margem_operacional: row.get(43)?,
                    roic: row.get(44)?,
                    net_debt: row.get(45)?,
                    divida_liquida_pl: row.get(46)?,
                    divida_liquida_ebit: row.get(47)?,
                    pl: row.get(48)?,
                    ev_ebit: row.get(49)?,
                    receita_ttm: row.get(50)?,
                    pvp_justo_5a: row.get(52)?,
                },
                g_usado: row.get(27)?,
                g_fonte: row.get(28)?,
                vp_por_acao: row.get(29)?,
                net_income_ttm: row.get(30)?,
                dividends_ttm: row.get(31)?,
                margem_liquida: row.get(32)?,
                margem_operacional: row.get(33)?,
                roic: row.get(34)?,
                net_debt: row.get(35)?,
                divida_liquida_pl: row.get(36)?,
                divida_liquida_ebit: row.get(37)?,
                pl: row.get(38)?,
                ev_ebit: row.get(39)?,
                receita_ttm: row.get(40)?,
                flags,
                warnings: Vec::new(),
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    // ── Settings ─────────────────────────────────────────────────────────────

    pub fn get_app_settings(&self) -> Result<AppSettings> {
        let mut s = AppSettings::default();
        let conn = self.lock();
        let mut stmt = conn.prepare("SELECT key, value FROM settings")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows.flatten() {
            match row.0.as_str() {
                "desired_return_rate" => {
                    if let Ok(v) = row.1.parse() { s.desired_return_rate = v; }
                }
                "fallback_growth_rate" => {
                    if let Ok(v) = row.1.parse() { s.fallback_growth_rate = v; }
                }
                "min_daily_liquidity" => {
                    if let Ok(v) = row.1.parse() { s.min_daily_liquidity = v; }
                }
                "selic_rate" => {
                    if let Ok(v) = row.1.parse() { s.selic_rate = v; }
                }
                "max_growth_rate" => {
                    if let Ok(v) = row.1.parse() { s.max_growth_rate = v; }
                }
                "min_dividend_yield" => {
                    if let Ok(v) = row.1.parse() { s.min_dividend_yield = v; }
                }
                _ => {}
            }
        }
        Ok(s)
    }

    pub fn save_app_settings(&self, s: &AppSettings) -> Result<()> {
        let conn = self.lock();
        let pairs = [
            ("desired_return_rate", s.desired_return_rate.to_string()),
            ("fallback_growth_rate", s.fallback_growth_rate.to_string()),
            ("min_daily_liquidity", s.min_daily_liquidity.to_string()),
            ("selic_rate", s.selic_rate.to_string()),
            ("max_growth_rate", s.max_growth_rate.to_string()),
            ("min_dividend_yield", s.min_dividend_yield.to_string()),
        ];
        for (k, v) in &pairs {
            conn.execute(
                "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
                params![k, v],
            )?;
        }
        Ok(())
    }
}

// ─── Caminho do banco ─────────────────────────────────────────────────────────

fn db_path() -> PathBuf {
    // CARGO_MANIFEST_DIR = <raiz do projeto>\src-tauri (compile-time)
    // O pai desse diretório é a raiz do projeto
    let manifest = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest)
        .parent()
        .map(|p| p.join("finance_data.db"))
        .unwrap_or_else(|| PathBuf::from("finance_data.db"))
}
