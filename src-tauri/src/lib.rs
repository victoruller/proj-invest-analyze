pub mod analysis_engine;
pub mod data_cache;
pub mod models;
pub mod y_finance_getter;

use std::sync::Arc;

use analysis_engine::compute_metrics;
use data_cache::DataCache;
use models::{AppSettings, StockSummary, WatchlistEntry};
use y_finance_getter::{build_client, fetch_full, fetch_incremental};
use yfinance_rs::YfClient;

// ─── Estado compartilhado da aplicação ───────────────────────────────────────

pub struct AppState {
    pub cache: Arc<DataCache>,
    pub yf_client: Arc<YfClient>,
}

/// Base de ajuste dos preços salvos. Mudar esta string força a re-coleta do histórico
/// completo no próximo refresh, para não misturar séries com ajustes diferentes.
const PRICE_BASIS: &str = "split_only_v1";

// ─── Helpers internos ─────────────────────────────────────────────────────────

/// Persiste todos os dados recebidos e recalcula métricas
async fn persist_and_compute(
    ticker: &str,
    data: y_finance_getter::FetchedData,
    cache: &Arc<DataCache>,
    settings: &AppSettings,
) -> Result<StockSummary, String> {
    // Persiste nome
    if let Some(ref name) = data.display_name {
        cache.update_display_name(ticker, name).map_err(|e| e.to_string())?;
    }

    // Persiste moedas (balanço vs. cotação) e o câmbio usado na conversão
    cache
        .update_currencies(ticker, data.fin_currency.as_deref(), data.quote_currency.as_deref())
        .map_err(|e| e.to_string())?;
    if let Some((ref fx_symbol, ref fx_candles)) = data.fx {
        cache.upsert_candles(fx_symbol, fx_candles).map_err(|e| e.to_string())?;
    }
    cache.update_quote_shares(ticker, data.quote_shares).map_err(|e| e.to_string())?;
    cache.update_price_basis(ticker, PRICE_BASIS).map_err(|e| e.to_string())?;

    // Persiste preços (UPSERT — não duplica)
    cache.upsert_candles(ticker, &data.candles).map_err(|e| e.to_string())?;

    // Persiste fundamentals (UPSERT — atualiza com revisões)
    cache.upsert_income_quarterly(ticker, &data.income_quarterly).map_err(|e| e.to_string())?;
    cache.upsert_income_annual(ticker, &data.income_annual).map_err(|e| e.to_string())?;
    cache.upsert_balance_quarterly(ticker, &data.balance_quarterly).map_err(|e| e.to_string())?;
    cache.upsert_balance_annual(ticker, &data.balance_annual).map_err(|e| e.to_string())?;
    cache.upsert_cashflow(ticker, &data.cashflow_quarterly, true).map_err(|e| e.to_string())?;
    cache.upsert_cashflow(ticker, &data.cashflow_annual, false).map_err(|e| e.to_string())?;
    cache.upsert_dividends(ticker, &data.dividends).map_err(|e| e.to_string())?;

    // Sem cotação as métricas sairiam degradadas (P/VP, DY e P/L dependem do preço):
    // preserva o metrics_cache e o last_fetched_at anteriores e devolve o que já havia.
    if data.current_price.is_none() {
        let mut warnings = data.warnings;
        warnings.push(
            "Preço atual indisponível — métricas e data de atualização anteriores mantidas".into(),
        );
        let previous = cache
            .get_all_summaries()
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|s| s.ticker == ticker)
            .ok_or_else(|| format!("Sem cotação para {ticker} e sem métricas no cache"))?;
        return Ok(StockSummary { warnings, ..previous });
    }

    // Atualiza timestamp de última coleta
    cache.update_last_fetched(ticker).map_err(|e| e.to_string())?;

    // Recupera metadados atualizado para o summary
    let entry = cache.get_watchlist()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|w| w.ticker == ticker)
        .ok_or_else(|| format!("Ticker {} não encontrado na watchlist", ticker))?;

    // Calcula métricas
    let mut summary = tokio::task::spawn_blocking({
        let ticker = ticker.to_string();
        let display_name = entry.display_name.clone();
        let current_price = data.current_price;
        let last_fetched_at = entry.last_fetched_at;
        let cache = Arc::clone(cache);
        let settings = settings.clone();
        move || {
            compute_metrics(
                &ticker,
                display_name.as_deref(),
                current_price,
                last_fetched_at,
                &cache,
                &settings,
            )
        }
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;

    summary.warnings = data.warnings;

    // Persiste métricas calculadas
    cache.upsert_metrics(&summary).map_err(|e| e.to_string())?;

    Ok(summary)
}

// ─── Comandos Tauri ───────────────────────────────────────────────────────────

/// Adiciona um ticker à watchlist e inicia a coleta completa (5 anos)
#[tauri::command]
async fn add_ticker(
    ticker: String,
    state: tauri::State<'_, AppState>,
) -> Result<StockSummary, String> {
    let ticker = ticker.trim().to_uppercase();
    if ticker.is_empty() {
        return Err("Ticker não pode ser vazio".into());
    }

    state.cache.insert_watchlist(&ticker, None).map_err(|e| e.to_string())?;

    let data = fetch_full(&state.yf_client, &ticker)
        .await
        .map_err(|e| e.to_string())?;

    let settings = state.cache.get_app_settings().map_err(|e| e.to_string())?;
    persist_and_compute(&ticker, data, &state.cache, &settings).await
}

/// Remove um ticker e todos os seus dados
#[tauri::command]
async fn remove_ticker(
    ticker: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    state.cache.remove_watchlist(&ticker).map_err(|e| e.to_string())
}

/// Retorna a lista de ativos monitorados
#[tauri::command]
async fn get_watchlist(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<WatchlistEntry>, String> {
    state.cache.get_watchlist().map_err(|e| e.to_string())
}

/// Atualização incremental: busca apenas os dados faltantes desde o último registro
#[tauri::command]
async fn refresh_ticker(
    ticker: String,
    state: tauri::State<'_, AppState>,
) -> Result<StockSummary, String> {
    // Histórico salvo com outra base de ajuste é re-coletado inteiro (last_ts = None)
    let basis_ok = state
        .cache
        .get_price_basis(&ticker)
        .map_err(|e| e.to_string())?
        .as_deref()
        == Some(PRICE_BASIS);
    let last_ts = if basis_ok {
        state.cache.last_price_ts(&ticker).map_err(|e| e.to_string())?
    } else {
        None
    };

    let data = fetch_incremental(&state.yf_client, &ticker, last_ts)
        .await
        .map_err(|e| e.to_string())?;

    // A re-coleta cobre só os últimos 5 anos: o trecho mais antigo que sobrou ficaria
    // na base de ajuste anterior, misturando as duas séries.
    if !basis_ok {
        if let Some(oldest) = data.candles.iter().map(|c| c.ts).min() {
            state
                .cache
                .delete_candles_before(&ticker, oldest)
                .map_err(|e| e.to_string())?;
        }
    }

    let settings = state.cache.get_app_settings().map_err(|e| e.to_string())?;
    persist_and_compute(&ticker, data, &state.cache, &settings).await
}

/// Retorna todos os summaries do cache (sem acesso à internet)
#[tauri::command]
async fn get_analysis(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<StockSummary>, String> {
    state.cache.get_all_summaries().map_err(|e| e.to_string())
}

/// Retorna as configurações atuais
#[tauri::command]
async fn get_settings(
    state: tauri::State<'_, AppState>,
) -> Result<AppSettings, String> {
    state.cache.get_app_settings().map_err(|e| e.to_string())
}

/// Salva as configurações e recalcula todas as métricas (sem nova coleta)
#[tauri::command]
async fn save_settings(
    settings: AppSettings,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<StockSummary>, String> {
    state.cache.save_app_settings(&settings).map_err(|e| e.to_string())?;

    let watchlist = state.cache.get_watchlist().map_err(|e| e.to_string())?;

    for entry in &watchlist {
        // Recalcula métricas usando dados já no cache — sem chamadas de rede
        let summary = {
            let ticker = entry.ticker.clone();
            let display_name = entry.display_name.clone();
            let last_fetched_at = entry.last_fetched_at;
            let cache = Arc::clone(&state.cache);
            let settings = settings.clone();
            tokio::task::spawn_blocking(move || {
                // Obtém preço atual do cache de métricas (sem reconectar)
                let current_price = cache
                    .get_all_summaries()
                    .ok()
                    .and_then(|v| v.into_iter().find(|s| s.ticker == ticker))
                    .and_then(|s| s.current_price);

                compute_metrics(
                    &ticker,
                    display_name.as_deref(),
                    current_price,
                    last_fetched_at,
                    &cache,
                    &settings,
                )
            })
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?
        };

        state.cache.upsert_metrics(&summary).map_err(|e| e.to_string())?;
    }

    state.cache.get_all_summaries().map_err(|e| e.to_string())
}

// ─── Bootstrap ────────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let cache = Arc::new(DataCache::open().expect("Falha ao abrir banco de dados"));
    let yf_client = Arc::new(build_client());

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState { cache, yf_client })
        .invoke_handler(tauri::generate_handler![
            add_ticker,
            remove_ticker,
            get_watchlist,
            refresh_ticker,
            get_analysis,
            get_settings,
            save_settings,
        ])
        .run(tauri::generate_context!())
        .expect("erro ao iniciar aplicação");
}
