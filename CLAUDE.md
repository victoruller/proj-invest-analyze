# CLAUDE.md

Guia para trabalhar neste repositório. Veja também `README.md` para visão geral do
produto e das fórmulas financeiras.

## O que é

App desktop (Tauri v2 + Rust + TypeScript vanilla) que analisa ações da B3 e classifica
cada ativo como `COMPRA` / `MANTER` / `VENDA` / `DADOS_INSUFICIENTES`, com base em
**P/VP atual vs. P/VP justo** (Gordon Growth Model: `(ROE - g) / (r - g)`). Toda a
lógica de negócio fica no backend Rust; o frontend é essencialmente uma camada de
apresentação que chama comandos Tauri via `invoke()`.

## Build — pré-requisito obrigatório: `protoc`

`yfinance-rs` usa `prost-build` para compilar `.proto` de streaming. **Sem `protoc`
no PATH ou na env var `PROTOC`, `cargo build`/`pnpm tauri dev` falham** com
`Could not find 'protoc'`.

No Windows deste ambiente, `protoc` foi instalado via winget e fica em:
```
PROTOC="/c/Users/victo/AppData/Local/Microsoft/WinGet/Packages/Google.Protobuf_Microsoft.Winget.Source_8wekyb3d8bbwe/bin/protoc.exe"
```
Setar essa env var antes de qualquer `cargo build`, `cargo check` ou `pnpm tauri dev`/`build`
neste projeto, a menos que já esteja no PATH da sessão.

## Comandos

```bash
pnpm install          # deps do frontend
pnpm tauri dev         # dev: Vite (localhost:1420) + app Tauri
pnpm tauri build       # build de produção
cargo check            # (dentro de src-tauri/) checagem rápida do backend Rust
```

Não há suíte de testes automatizada no projeto (nem Rust `#[test]` nem testes de
frontend) até o momento. Validar mudanças rodando `pnpm tauri dev` e testando a
funcionalidade manualmente na UI.

## Arquitetura

```
src/main.ts                    Frontend: watchlist, tabela, tooltips, tela de settings
src-tauri/src/
  lib.rs                       Comandos Tauri (#[tauri::command]) + orquestração
  y_finance_getter.rs          Fetch de dados via yfinance-rs (fetch_full / fetch_incremental)
  data_cache.rs                SQLite (rusqlite): schema, upserts, queries
  analysis_engine.rs           Cálculo de métricas + classify() (COMPRA/MANTER/VENDA)
  models.rs                    Structs compartilhados (serde) entre backend e frontend
```

Fluxo de dados: `add_ticker`/`refresh_ticker` (lib.rs) → `y_finance_getter` busca dados
do Yahoo Finance → `data_cache` persiste em SQLite (upsert) → `analysis_engine::compute_metrics`
lê do cache (não recebe os dados brutos direto) e calcula tudo → resultado (`StockSummary`)
é salvo de volta no cache (`metrics_cache`) e retornado ao frontend.

Importante: `compute_metrics` **sempre lê do SQLite**, nunca do `FetchedData` recém-buscado.
Isso significa que qualquer novo campo em `FetchedData`/tabelas do banco só é visível
para o motor de cálculo depois de passar por um `upsert_*` em `data_cache.rs`.

## Convenções do domínio

- Tickers da B3 recebem sufixo `.SA` automaticamente (`b3_symbol()` em
  `y_finance_getter.rs`) antes de consultar o Yahoo Finance. Tickers são normalizados
  para uppercase.
- TTM = Trailing Twelve Months. `calc_ttm()` só soma os 4 últimos trimestres se forem
  consecutivos (span ≤ 300 dias) e todos tiverem o campo; senão cai para o último
  exercício anual. Somar trimestres esparsos distorcia ROE e margens (ABEV3 tinha
  trimestres nulos e ficava com ROE de 6,8% em vez de ~17%).
- O `g` do Gordon é **crescimento perpétuo**: `compute_g()` limita o CAGR a
  `[0, settings.max_growth_rate]`. Sem o teto, um CAGR próximo de `r` explodia o
  P/VP justo (WEGE3 chegou a 124x com g=14,86% e r=15%).
- `pvp_justo` (ROE TTM) e `pvp_justo_5a` (ROE médio 5a) usam o **mesmo** `g` e são
  comparados ambos ao **P/VP atual** — as duas situações decidem sobre o preço de hoje.
  `pvp_medio_5a` é informativo e não entra em `classify()`.
- "5 anos" nas médias de P/VP e ROE = anos calendário. Já o DY médio 5a usa 5 janelas
  móveis de 12 meses (contar o ano corrente pela metade subestimava o DY).
- Preços em `price_history` são ajustados **só por splits** (`auto_adjust(false)`). O
  padrão do crate ajusta também por dividendos, o que deflaciona o passado (PETR4 em
  jan/2022: 9,66 vs. 29,09) e triplicava o DY histórico. `watchlist.price_basis` grava a
  base usada; quando não bate com `PRICE_BASIS` (lib.rs), o refresh re-coleta os 5 anos e
  chama `delete_candles_before()` para apagar o trecho anterior à janela — senão sobraria
  um pedaço na base antiga (os tickers tinham 1372 pregões contra 1248 da janela nova).
- Cada métrica em `analysis_engine.rs` retorna uma tupla `(Option<f64>, Option<String>)`:
  o valor e um JSON de detalhamento (fórmula + números usados), exibido como tooltip
  no frontend via `MetricDetails`. Ao adicionar uma métrica nova, siga esse padrão.
- Toda métrica lida com dados ausentes retornando `None` em vez de erro — nunca deixe
  `compute_metrics` falhar por falta de dado de um trimestre/balanço específico.
- `g` (crescimento) é CAGR do lucro líquido anual (até 5 anos); cai para
  `settings.fallback_growth_rate` se houver <2 anos de dados ou lucro negativo em
  algum ano (`compute_g()` em `analysis_engine.rs`).
- `classify()` é chamada duas vezes por ativo: uma com métricas atuais
  (`situacao_atual`) e outra com médias de 5 anos (`situacao_5a`). Mudanças nas
  regras de classificação afetam ambas.
- COMPRA **não** exige DY por padrão (`settings.min_dividend_yield` = 0). A regra antiga
  era `DY > r/2`, que contava o dividendo duas vezes: `(ROE - g)/(r - g)` já assume
  payout de `1 - g/ROE`. Além de barrar empresas de crescimento, amarrar o limiar ao `r`
  fazia um ajuste no retorno exigido apertar o filtro de renda junto, sem querer.
- `calc_pvp_justo()` rejeita `ROE ≤ g` (justo ≤ 0, crescimento que não se sustenta) além
  de `g ≥ r` e `ROE ≤ 0`.

## Moedas (armadilha principal dos dados)

O Yahoo entrega balanços em moeda diferente da cotação: PETR4/VALE3 reportam em USD com
cotação em BRL, BDRs reportam em USD/TWD. O `yfinance-rs` **não ajuda aqui** — ele infere
a moeda pelo país da empresa (PETR4 → BRL, errado) e `Info.eps_ttm` é sempre `None`.

A moeda real vem do `currencyCode` do endpoint de fundamentals do Yahoo, que o crate
ignora; por isso `y_finance_getter::fetch_statement_currency()` chama esse endpoint
direto com `reqwest` (sem crumb/autenticação). O câmbio é buscado como um ticker comum
(`USDBRL=X`) e guardado em `price_history`, para o motor continuar lendo só do SQLite.

Para BDRs falta ainda a proporção BDR/ação: o preço é por BDR e o balanço é por ação.
Ela sai de `defaultKeyStatistics.sharesOutstanding` do `quoteSummary` (que para o ticker
do BDR vem em BDRs) dividido pelas ações do balanço — AAPL34: 291,88 bi / 14,69 bi ≈
19,87. Esse endpoint exige cookie + crumb: `yahoo_session()` replica o fluxo do yfinance
(GET `fc.yahoo.com`, que responde 404 mas grava o cookie, depois `getcrumb`) e guarda a
sessão num `OnceCell`. Use `sharesOutstanding`, não `impliedSharesOutstanding` (difere em
empresas com várias classes).

**Nunca aplique essa proporção a ações comuns**: nelas o Yahoo conta só a classe cotada
(PETR4 = 5,4 bi PN vs. 12,9 bi ON+PN no balanço), o que daria um fator errado de 0,42.
E **não arredonde** a proporção: TSMC34 é 1,60 de verdade (BDR sobre ADR, e 1 ADR = 5
ações), então arredondar para 2 erraria 20%.

No motor, a struct `Units` centraliza isso:
- `share_fx()` → fator para valores POR AÇÃO: câmbio dividido pela proporção BDR/ação.
  Retorna `None` quando falta câmbio ou, em BDR, a proporção — aí P/VP, P/L e EV/EBIT
  ficam indisponíveis com motivo explícito em vez de número errado.
- `to_quote()` → converte totais (lucro, receita, dívida) para exibição.
- Razões puras (ROE, ROIC, margens, D/PL, D/EBIT) não precisam de conversão.

Ao mexer em qualquer métrica que compare preço com balanço, passe por `Units` — senão o
número volta a misturar BRL com USD silenciosamente.

## Persistência

- Banco: `finance_data.db` (SQLite, WAL mode) na raiz do projeto. Os arquivos
  `finance_data.db`/`.db-shm`/`.db-wal` **não estão no `.gitignore` atualmente** —
  cuidado para não commitar acidentalmente esses arquivos ao rodar `git add -A`.
- Tabelas: `watchlist`, `price_history`, `income_quarterly/annual`,
  `balance_quarterly/annual`, `cashflow_quarterly/annual`, `dividends`, `settings`,
  `metrics_cache`. Todas as tabelas de fundamentals usam `UPSERT` por
  `(ticker, period_label)` — reexecutar fetch não duplica linhas, apenas atualiza
  revisões.
- `save_settings` recalcula métricas de todos os ativos da watchlist **sem** nova
  chamada de rede (usa preço já salvo em `metrics_cache`).
- Coleta sem cotação **não** grava: `persist_and_compute()` retorna cedo, preservando
  `metrics_cache` e `last_fetched_at` anteriores. P/VP, DY e P/L dependem do preço, então
  gravar ali trocaria dados bons por uma linha degradada. Candles e fundamentals, que não
  dependem do preço, continuam sendo persistidos.
- Fontes que falham viram avisos, não silêncio: `take()` em `y_finance_getter.rs` acumula
  cada falha parcial em `FetchedData::warnings`, repassado ao frontend em
  `StockSummary::warnings` (campo transitório, não persistido — `get_all_summaries()` o
  devolve vazio). Nunca troque isso de volta por `unwrap_or_default()`.

## Ao mexer no motor de cálculo

- Fórmulas e regras de classificação estão comentadas extensivamente em
  `analysis_engine.rs` (comentários de seção com `// ─── N. Nome ───`). Leia o
  comentário da seção antes de alterar a lógica — geralmente documenta a fonte dos
  dados e o motivo de casos de borda retornarem `None`.
- Se alterar a fórmula do P/VP justo ou as regras de `classify()`, atualize a
  explicação equivalente no `README.md`, que documenta isso para usuários finais.
- Novo campo em `StockSummary` requer mudança em 3 lugares: `models.rs` (struct +
  `MetricDetails` se tiver tooltip), `analysis_engine.rs` (cálculo) e
  `src/main.ts` (interface TS espelhando o struct + renderização na tabela).
