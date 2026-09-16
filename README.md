# B3 Finance Analyser

Aplicativo desktop (Tauri + Rust + TypeScript) para análise fundamentalista de ações da
B3, focado em decidir se um ativo está em momento de **compra**, **manter** ou **venda**
com base no **P/VP (Preço sobre Valor Patrimonial)** atual comparado ao **P/VP justo**
calculado pelo modelo de crescimento de Gordon.

## Como funciona

1. Você adiciona um ticker (ex: `VALE3`, `ITSA4`) a uma watchlist.
2. O app busca no Yahoo Finance (via [`yfinance-rs`](https://crates.io/crates/yfinance-rs))
   5 anos de histórico de preços, dividendos e demonstrações financeiras (DRE, balanço)
   trimestrais e anuais.
3. Os dados são persistidos em SQLite local (`finance_data.db`) para permitir
   atualizações incrementais sem reconsultar tudo.
4. Um motor de cálculo (`analysis_engine.rs`) deriva um conjunto de métricas
   fundamentalistas e classifica o ativo em `COMPRA`, `MANTER`, `VENDA` ou
   `DADOS_INSUFICIENTES`.

## A métrica central: P/VP Justo

O modelo usado é o **Gordon Growth Model** aplicado ao patrimônio:

```
P/VP_justo = (ROE - g) / (r - g)
```

- **ROE**: retorno sobre patrimônio TTM (lucro dos últimos 4 trimestres consecutivos /
  patrimônio líquido médio dos últimos 4 trimestres). Se faltarem trimestres, usa o
  último exercício anual.
- **g**: crescimento perpétuo, obtido pelo CAGR do lucro líquido anual (até 5 anos) e
  **limitado ao intervalo [0%, g máximo configurável]**. Sem dados suficientes ou com
  lucro negativo em algum ano, cai para a taxa de fallback configurável.
- **r**: taxa de retorno mínima desejada pelo investidor (configurável, padrão 12%).

O ativo é considerado:
- **subavaliado** (possível compra) quando `P/VP atual < P/VP justo × 0.9`;
- **sobreavaliado** (possível venda) quando `P/VP atual > P/VP justo × 1.2`;
- **em linha com o justo** (manter) na faixa intermediária.

A classificação final (`classify` em `analysis_engine.rs`) também exige liquidez diária
mínima e ROE mínimo antes de sinalizar `COMPRA`, e sinaliza `VENDA` diretamente em caso
de ROE negativo ou liquidez muito baixa.

O **Dividend Yield não é exigido por padrão**: a fórmula já embute o dividendo, porque o
modelo assume que a empresa retém apenas o necessário para crescer `g` e distribui o
resto (payout implícito = `1 - g/ROE`). Exigir DY alto por cima disso contaria o
dividendo duas vezes e barraria empresas de crescimento. Quem quiser um filtro de renda
explícito pode configurar `min_dividend_yield`.

### Situação Atual vs. Situação 5a

As duas colunas respondem à mesma pergunta — *vale a pena comprar pelo preço de hoje?* —
e ambas usam o **P/VP atual**. O que muda é o ROE que alimenta o P/VP justo:

| Coluna           | Compara            | Contra              | ROE usado    | DY usado    |
|------------------|--------------------|---------------------|--------------|-------------|
| `Situação Atual` | P/VP atual (hoje)  | `P/VP Justo`        | ROE TTM      | DY TTM      |
| `Situação 5a`    | P/VP atual (hoje)  | `P/VP Justo 5a`     | ROE médio 5a | DY médio 5a |

A `Situação 5a` existe para que um trimestre (ou ano) excepcionalmente bom ou ruim não
domine a decisão. A coluna `P/VP Méd. 5a` é apenas informativa — mostra como o mercado
precificou o ativo historicamente e **não** entra na classificação.

## Moedas e BDRs

O Yahoo Finance entrega alguns balanços em moeda diferente da cotação: PETR4 e VALE3
reportam em **USD** enquanto a cotação vem em BRL. Sem tratar isso, o P/VP da PETR4
sairia ~7,0 em vez de ~1,3. O app detecta a moeda real de cada balanço pelo campo
`currencyCode` do endpoint de fundamentals do Yahoo e converte os valores usando o
histórico do par de câmbio correspondente (ex.: `USDBRL=X`), guardado no mesmo cache de
preços.

Para **BDRs** (tickers terminados em 31–35 ou 39, como `AAPL34`) a conversão de moeda não
basta: o preço é por BDR, enquanto o balanço é por ação da empresa. A proporção entre os
dois é obtida comparando as ações que o Yahoo reporta para o ticker do BDR — que vêm em
BDRs — com as ações do balanço. Para AAPL34 isso dá 291,88 bi / 14,69 bi ≈ **19,87 BDRs
por ação** (a proporção real é 20).

A proporção medida é usada como está, **sem arredondamento**: BDRs sobre ADRs têm
proporções fracionárias legítimas (TSMC34 dá exatamente 1,60, porque cada ADR equivale a
5 ações ordinárias), e arredondar criaria erro. Quando o Yahoo não informa as ações em
circulação, as métricas por ação continuam indisponíveis em vez de receberem um palpite.

Essa proporção vale **apenas para BDRs**. Em ações comuns o Yahoo conta só a classe
cotada (PETR4: 5,4 bi preferenciais contra 12,9 bi ON+PN no balanço), então ações seguem
convertidas somente pelo câmbio.

Outras métricas complementares calculadas: DY (dividend yield TTM), margem líquida
e operacional, ROIC, dívida líquida (e seus múltiplos sobre PL e EBIT), P/L e
EV/EBIT. Cada métrica carrega também um "detail" em JSON com a fórmula e os números
usados no cálculo, exibido como tooltip no frontend.

## Stack

- **Backend**: Rust + [Tauri v2](https://tauri.app/), SQLite via `rusqlite`
  (`bundled`), `yfinance-rs` para dados de mercado, `tokio` para I/O assíncrono.
- **Frontend**: TypeScript vanilla + Vite, sem framework (comunica com o backend via
  `invoke()` do Tauri).
- **Persistência**: `finance_data.db` (SQLite, arquivo local, gerado em runtime).

## Setup de desenvolvimento

### Pré-requisitos

- [Rust](https://rustup.rs/) (edition 2021)
- [Node.js](https://nodejs.org/) + [pnpm](https://pnpm.io/)
- **`protoc`** (Protocol Buffers compiler) — necessário porque `yfinance-rs` compila
  `.proto` de streaming via `prost-build`. Sem ele o `cargo build` falha com
  `Could not find 'protoc'`.
  - Windows: `winget install protobuf --accept-package-agreements --accept-source-agreements`
  - Depois, aponte a variável de ambiente `PROTOC` para o executável instalado, ex.:
    ```
    PROTOC="/c/Users/<user>/AppData/Local/Microsoft/WinGet/Packages/Google.Protobuf_Microsoft.Winget.Source_8wekyb3d8bbwe/bin/protoc.exe"
    ```

### Rodando

```bash
pnpm install
pnpm tauri dev
```

O comando acima sobe o Vite em `http://localhost:1420` e o app Tauri, seguindo a
config em `src-tauri/tauri.conf.json`.

### Build de produção

```bash
pnpm tauri build
```

## Estrutura do projeto

```
src/                       Frontend (TypeScript vanilla)
  main.ts                  Toda a lógica de UI: watchlist, tabela de métricas, tooltips, settings
  styles.css

src-tauri/src/
  lib.rs                   Comandos Tauri (invoke handlers) e orquestração de fetch + cálculo + persistência
  y_finance_getter.rs       Integração com yfinance-rs: fetch completo (5a) e incremental
  data_cache.rs             Camada SQLite: schema, upserts, queries de série histórica
  analysis_engine.rs        Motor de cálculo das métricas fundamentalistas e classificação
  models.rs                 Structs compartilhados (AppSettings, StockSummary, rows internas)
```

## Comandos Tauri expostos ao frontend

| Comando          | Descrição                                                              |
|------------------|-------------------------------------------------------------------------|
| `add_ticker`     | Adiciona ticker à watchlist e faz fetch completo (5 anos)               |
| `remove_ticker`  | Remove ticker e todos os seus dados                                     |
| `get_watchlist`  | Lista ativos monitorados                                                |
| `refresh_ticker` | Fetch incremental (só dados novos desde o último preço salvo)           |
| `get_analysis`   | Retorna todos os summaries já calculados, sem acesso à rede             |
| `get_settings`   | Retorna configurações atuais (`r`, `g` fallback, liquidez mínima, Selic)|
| `save_settings`  | Salva configurações e recalcula métricas de todos os ativos (sem fetch) |

## Configurações do usuário (`AppSettings`)

| Campo                  | Padrão   | Significado                                              |
|-------------------------|----------|-----------------------------------------------------------|
| `desired_return_rate`   | `0.12`   | `r` no modelo de Gordon                                    |
| `fallback_growth_rate`  | `0.05`   | `g` usado quando CAGR não pode ser calculado               |
| `max_growth_rate`       | `0.06`   | Teto do `g` vindo do CAGR (crescimento perpétuo)           |
| `min_dividend_yield`    | `0.0`    | DY mínimo para COMPRA; 0 = sem filtro de renda             |
| `min_daily_liquidity`   | `200000` | Liquidez diária mínima em R$ (Close × Volume médio 252d)   |
| `selic_rate`            | `0.1075` | Apenas referência visual, não entra em nenhum cálculo      |

## Limitações conhecidas

- **BDRs**: sem proporção BDR/ação, o valuation por ação fica indisponível (ver acima).
- **Dívida líquida** usa apenas dívida de longo prazo — o feed do Yahoo via `yfinance-rs`
  não traz dívida de curto prazo, então o endividamento é subestimado.
- **EV/EBIT** é usado no lugar de EV/EBITDA porque D&A não está disponível no feed.
- **Fundamentals podem vir incompletos** (trimestres nulos ou faltando); nesses casos o
  TTM cai para o último exercício anual, o que é sinalizado no tooltip da métrica.
- **Coleta parcial é avisada, não silenciada**: se alguma fonte do Yahoo falhar (DRE,
  balanço, dividendos, câmbio…), o ativo é atualizado com o que veio e os problemas
  aparecem em amarelo na linha do ativo, na aba Ativos. E se a **cotação** não vier, as
  métricas e a data de atualização anteriores são preservadas em vez de sobrescritas por
  uma linha degradada — P/VP, DY e P/L dependem do preço.
- **Câmbio histórico**: o P/VP médio de 5 anos usa o câmbio médio de cada ano, mas o P/VP
  atual e o de 1 ano usam o câmbio mais recente.
- **Ajuste de preços**: o histórico é coletado ajustado apenas por **splits**, nunca por
  dividendos. O padrão do `yfinance-rs` ajusta por dividendos, o que deflaciona os preços
  passados (PETR4 em jan/2022: 9,66 em vez de 29,09) e inflaria o DY histórico. Ativos
  coletados antes dessa mudança têm o histórico de 5 anos re-coletado automaticamente no
  próximo "Atualizar" (controlado pela coluna `watchlist.price_basis`); pregões anteriores
  a essa janela de 5 anos são descartados, para não deixar um trecho na base antiga.

## Aviso

Este projeto é uma ferramenta de apoio à análise, não recomendação de investimento.
Os cálculos dependem da qualidade e disponibilidade dos dados fornecidos pelo Yahoo
Finance via `yfinance-rs`, que podem estar incompletos ou desatualizados para
algumas ações da B3.
