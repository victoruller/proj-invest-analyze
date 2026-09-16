import { invoke } from "@tauri-apps/api/core";

// ─── Tipos (espelham os structs Rust) ────────────────────────────────────────

interface AppSettings {
  desired_return_rate: number;
  fallback_growth_rate: number;
  min_daily_liquidity: number;
  selic_rate: number;
  max_growth_rate: number;
  min_dividend_yield: number;
}

interface MetricDetails {
  liquidez?: string;
  pvp_atual?: string;
  roe_atual?: string;
  dy_atual?: string;
  pvp_justo?: string;
  pvp_justo_5a?: string;
  pvp_medio_1a?: string;
  pvp_medio_5a?: string;
  roe_medio_5a?: string;
  dy_medio_5a?: string;
  g_usado?: string;
  // Novas métricas
  receita_ttm?: string;
  margem_liquida?: string;
  margem_operacional?: string;
  roic?: string;
  net_debt?: string;
  divida_liquida_pl?: string;
  divida_liquida_ebit?: string;
  pl?: string;
  ev_ebit?: string;
}

interface StockSummary {
  ticker: string;
  display_name?: string;
  current_price?: number;
  liquidez_diaria?: number;
  pvp_atual?: number;
  roe_atual?: number;
  dy_atual?: number;
  pvp_justo?: number;
  pvp_justo_5a?: number;
  pvp_medio_1a?: number;
  pvp_medio_5a?: number;
  roe_medio_5a?: number;
  dy_medio_5a?: number;
  // Extras
  g_usado?: number;
  g_fonte?: string;
  vp_por_acao?: number;
  net_income_ttm?: number;
  dividends_ttm?: number;
  // Novas métricas financeiras
  receita_ttm?: number;
  margem_liquida?: number;
  margem_operacional?: number;
  roic?: number;
  net_debt?: number;
  divida_liquida_pl?: number;
  divida_liquida_ebit?: number;
  pl?: number;
  ev_ebit?: number;
  flags: string[];
  warnings: string[];
  situacao_atual: string;
  situacao_atual_reason: string;
  situacao_5a: string;
  situacao_5a_reason: string;
  last_fetched_at?: number;
  details: MetricDetails;
}

interface WatchlistEntry {
  ticker: string;
  display_name?: string;
  added_at: number;
  last_fetched_at?: number;
}

// ─── Colunas disponíveis ──────────────────────────────────────────────────────

const ALL_COLUMNS = [
  "ticker", "nome", "preco", "liquidez",
  "pvp_atual", "pvp_medio_1a", "pvp_medio_5a", "pvp_justo", "pvp_justo_5a",
  "roe_atual", "roe_medio_5a",
  "dy_atual", "dy_medio_5a",
  "g_usado", "vp_por_acao", "net_income_ttm", "dividends_ttm",
  "receita_ttm", "margem_liquida", "margem_operacional", "roic",
  "net_debt", "divida_liquida_pl", "divida_liquida_ebit", "pl", "ev_ebit",
  "flags",
  "situacao_atual", "situacao_5a", "atualizado",
] as const;

type ColKey = typeof ALL_COLUMNS[number];

const COL_LABELS: Record<ColKey, string> = {
  ticker: "Ticker",
  nome: "Nome",
  preco: "Preço",
  liquidez: "Liquidez Diária",
  pvp_atual: "P/VP Atual",
  pvp_medio_1a: "P/VP Méd. 1a",
  pvp_medio_5a: "P/VP Méd. 5a",
  pvp_justo: "P/VP Justo",
  pvp_justo_5a: "P/VP Justo 5a",
  roe_atual: "ROE Atual",
  roe_medio_5a: "ROE Méd. 5a",
  dy_atual: "DY Atual",
  dy_medio_5a: "DY Méd. 5a",
  g_usado: "Taxa g",
  vp_por_acao: "VP/Ação",
  net_income_ttm: "Lucro Líq. TTM",
  dividends_ttm: "Div. TTM",
  receita_ttm: "Receita TTM",
  margem_liquida: "Margem Líq.",
  margem_operacional: "Margem Op.",
  roic: "ROIC",
  net_debt: "Dívida Líquida",
  divida_liquida_pl: "D.Líq./P.L.",
  divida_liquida_ebit: "D.Líq./EBIT",
  pl: "P/L",
  ev_ebit: "EV/EBIT",
  flags: "Flags",
  situacao_atual: "Situação Atual",
  situacao_5a: "Situação 5a",
  atualizado: "Atualizado em",
};

const COL_DESCRIPTIONS: Record<ColKey, string> = {
  ticker: "Código do ativo na B3.",
  nome: "Nome da empresa.",
  preco: "Preço atual da ação (R$).",
  liquidez: "Volume financeiro médio DIÁRIO = média(Close × Volume) nos últimos 252 pregões (~12 meses).",
  pvp_atual: "Preço / Valor Patrimonial por ação. VP do balanço trimestral mais recente, convertido para a moeda da cotação quando o balanço vem em outra moeda (PETR4/VALE3 reportam em USD) e, em BDRs, dividido pela proporção BDR/ação (AAPL34 ≈ 19,87 BDRs por ação).",
  pvp_medio_1a: "Média do P/VP diário nos últimos 252 pregões (~12 meses). Usa o VP do balanço mais recente.",
  pvp_medio_5a: "INFORMATIVO (não entra na classificação): média do P/VP ANUAL dos últimos 5 anos. Para cada ano: avg(preços do ano) / VP anual.",
  pvp_justo: "P/VP Justo pelo Modelo de Gordon usando o ROE ATUAL (TTM): (ROE – g) / (r – g). r e g em % ao ano.",
  pvp_justo_5a: "P/VP Justo pelo Modelo de Gordon usando o ROE MÉDIO de 5 anos: (ROE méd. 5a – g) / (r – g). Menos sensível a um ano excepcionalmente bom ou ruim.",
  roe_atual: "ROE TTM = lucro dos 4 trimestres consecutivos / média(PL 4 trimestres). Se faltarem trimestres, usa o último exercício anual.",
  roe_medio_5a: "Média do ROE ANUAL dos últimos 5 anos = média(lucro_ano / PL_ano).",
  dy_atual: "Dividend Yield TTM = Σ(dividendos 12 meses) / preço atual. Base ANUAL (12 meses).",
  dy_medio_5a: "Média do DY de 5 janelas de 12 meses até hoje = média(Σdivs da janela / preço médio da janela).",
  g_usado: "Taxa de crescimento perpétuo g usada nos dois P/VP Justos. ANUAL. CAGR do lucro histórico limitado entre 0% e o g máximo configurado, ou taxa de fallback.",
  vp_por_acao: "Valor Patrimonial por ação = Patrimônio Líquido / Ações emitidas (balanço mais recente).",
  net_income_ttm: "Lucro Líquido TTM = soma dos últimos 4 trimestres (12 meses).",
  dividends_ttm: "Total de dividendos por ação pagos nos últimos 12 meses (R$ / ação).",
  receita_ttm: "Receita Líquida TTM = soma dos últimos 4 trimestres (12 meses).",
  margem_liquida: "Margem Líquida TTM = Lucro Líquido TTM / Receita TTM. Mede eficiência na conversão de receita em lucro.",
  margem_operacional: "Margem Operacional TTM = EBIT TTM / Receita TTM. EBIT = lucro antes de juros e impostos.",
  roic: "ROIC ANUAL = NOPAT / Capital Investido. NOPAT = EBIT × 0,66 (alíquota estimada 34% para BR). Capital Investido = PL + Dívida LP – Caixa.",
  net_debt: "Dívida Líquida = Dívida LP – Caixa. Negativo significa posição de caixa líquida (mais caixa do que dívida).",
  divida_liquida_pl: "Dívida Líquida / Patrimônio Líquido. Mede o grau de alavancagem em relação ao capital próprio.",
  divida_liquida_ebit: "Dívida Líquida / EBIT TTM. Proxy de D/EBITDA (D&A não disponível via Yahoo Finance). Alerta se > 3,0×.",
  pl: "P/L = Preço / LPA TTM. LPA = Lucro Líquido TTM / ações. Quanto o mercado paga por R$1 de lucro.",
  ev_ebit: "EV/EBIT = (Market Cap + Dívida Líquida) / EBIT TTM. EV/EBIT usado pois D&A não está disponível no feed do Yahoo Finance.",
  flags: "Alertas e oportunidades detectados automaticamente: 🚩 endividamento alto, ⭐ ROIC > 15% e subavaliada.",
  situacao_atual: "Preço de HOJE (P/VP atual) vs P/VP Justo pelo ROE ATUAL (TTM), com filtros de ROE e liquidez. O DY só filtra se você configurar um DY mínimo.",
  situacao_5a: "Preço de HOJE (P/VP atual) vs P/VP Justo 5a (ROE médio 5a). Mesma decisão de hoje, com fundamentos suavizados por 5 anos.",
  atualizado: "Data e hora da última atualização dos dados via Yahoo Finance.",
};

// Colunas visíveis por padrão
const DEFAULT_VISIBLE = new Set<ColKey>([
  "ticker", "nome", "preco", "liquidez",
  "pvp_atual", "pvp_justo", "pvp_justo_5a",
  "roe_atual", "dy_atual",
  "situacao_atual", "situacao_5a",
]);

const PREF_KEY = "b3_column_prefs";

// ─── Estado global ────────────────────────────────────────────────────────────

let visibleCols: Set<ColKey>;
let summaries: StockSummary[] = [];
let sortCol: ColKey | null = null;
let sortDir: "asc" | "desc" = "asc";

// ─── Init ─────────────────────────────────────────────────────────────────────

window.addEventListener("DOMContentLoaded", async () => {
  showSplash();
  loadColumnPrefs();
  setupTabs();
  setupTooltip();
  initColumnToggles();
  setupWatchlistTab();
  setupSettingsForm();
  setupHeaderTooltips();
  setupSortableHeaders();
  await Promise.all([
    loadAnalysis(),
    new Promise<void>(res => setTimeout(res, 2000)),
  ]);
  hideSplash();
});

// ─── Splash ───────────────────────────────────────────────────────────────────

function showSplash() {
  const fill = document.getElementById("splash-fill");
  if (!fill) return;
  setTimeout(() => { fill.style.width = "35%"; }, 150);
  setTimeout(() => { fill.style.width = "65%"; }, 900);
  setTimeout(() => { fill.style.width = "88%"; }, 1600);
}

function hideSplash() {
  const fill = document.getElementById("splash-fill");
  if (fill) fill.style.width = "100%";
  setTimeout(() => {
    const splash = document.getElementById("splash");
    if (!splash) return;
    splash.classList.add("fade-out");
    setTimeout(() => splash.remove(), 580);
  }, 250);
}

// ─── Abas ─────────────────────────────────────────────────────────────────────

function setupTabs() {
  document.querySelectorAll<HTMLButtonElement>(".tab-btn").forEach((btn) => {
    btn.addEventListener("click", () => {
      const tab = btn.dataset.tab!;
      document.querySelectorAll(".tab-btn").forEach((b) => b.classList.remove("active"));
      document.querySelectorAll(".tab-content").forEach((s) => s.classList.remove("active"));
      btn.classList.add("active");
      document.getElementById(`tab-${tab}`)?.classList.add("active");

      if (tab === "ativos") loadWatchlist();
      if (tab === "configuracoes") loadSettings();
    });
  });
}

// ─── Aba Análise ──────────────────────────────────────────────────────────────

async function loadAnalysis() {
  setStatus("Carregando dados…");
  try {
    summaries = await invoke<StockSummary[]>("get_analysis");
    renderTable(summaries);
    setStatus("");
  } catch (e) {
    setStatus(`Erro: ${e}`, true);
  }
}

function renderTable(data: StockSummary[]) {
  const tbody = document.getElementById("analysis-tbody")!;
  const emptyMsg = document.getElementById("empty-msg")!;
  const table = document.getElementById("analysis-table")!;

  if (data.length === 0) {
    table.style.display = "none";
    emptyMsg.style.display = "block";
    return;
  }

  table.style.display = "";
  emptyMsg.style.display = "none";
  tbody.innerHTML = "";

  for (const s of sortSummaries(data)) {
    tbody.appendChild(buildRow(s));
  }

  applyColumnVisibility();
}

// ─── Ordenação ────────────────────────────────────────────────────────────────

function getSortValue(s: StockSummary, col: ColKey): number | string | null {
  switch (col) {
    case "ticker":             return s.ticker;
    case "nome":               return s.display_name ?? "";
    case "preco":              return s.current_price ?? null;
    case "liquidez":           return s.liquidez_diaria ?? null;
    case "pvp_atual":          return s.pvp_atual ?? null;
    case "pvp_medio_1a":       return s.pvp_medio_1a ?? null;
    case "pvp_medio_5a":       return s.pvp_medio_5a ?? null;
    case "pvp_justo":          return s.pvp_justo ?? null;
    case "pvp_justo_5a":       return s.pvp_justo_5a ?? null;
    case "roe_atual":          return s.roe_atual ?? null;
    case "roe_medio_5a":       return s.roe_medio_5a ?? null;
    case "dy_atual":           return s.dy_atual ?? null;
    case "dy_medio_5a":        return s.dy_medio_5a ?? null;
    case "g_usado":            return s.g_usado ?? null;
    case "vp_por_acao":        return s.vp_por_acao ?? null;
    case "net_income_ttm":     return s.net_income_ttm ?? null;
    case "dividends_ttm":      return s.dividends_ttm ?? null;
    case "receita_ttm":        return s.receita_ttm ?? null;
    case "margem_liquida":     return s.margem_liquida ?? null;
    case "margem_operacional": return s.margem_operacional ?? null;
    case "roic":               return s.roic ?? null;
    case "net_debt":           return s.net_debt ?? null;
    case "divida_liquida_pl":  return s.divida_liquida_pl ?? null;
    case "divida_liquida_ebit":return s.divida_liquida_ebit ?? null;
    case "pl":                 return s.pl ?? null;
    case "ev_ebit":            return s.ev_ebit ?? null;
    case "situacao_atual":     return s.situacao_atual;
    case "situacao_5a":        return s.situacao_5a;
    case "atualizado":         return s.last_fetched_at ?? null;
    case "flags":              return s.flags.length;
    default:                   return null;
  }
}

function sortSummaries(data: StockSummary[]): StockSummary[] {
  if (!sortCol) return data;
  const col = sortCol;
  return [...data].sort((a, b) => {
    const av = getSortValue(a, col);
    const bv = getSortValue(b, col);
    // nulls always sink to the bottom regardless of direction
    if (av === null && bv === null) return 0;
    if (av === null) return 1;
    if (bv === null) return -1;
    let cmp: number;
    if (typeof av === "string" && typeof bv === "string") {
      cmp = av.localeCompare(bv, "pt-BR");
    } else {
      cmp = (av as number) - (bv as number);
    }
    return sortDir === "asc" ? cmp : -cmp;
  });
}

function setupSortableHeaders() {
  document.querySelectorAll<HTMLElement>("#analysis-table th[data-col]").forEach((th) => {
    const col = th.dataset.col as ColKey;
    th.addEventListener("click", () => {
      // Update sort state
      if (sortCol === col) {
        sortDir = sortDir === "asc" ? "desc" : "asc";
      } else {
        sortCol = col;
        sortDir = "asc";
      }
      // Update header indicators
      document.querySelectorAll<HTMLElement>("#analysis-table th[data-col]").forEach((h) => {
        h.classList.remove("sort-asc", "sort-desc");
      });
      th.classList.add(sortDir === "asc" ? "sort-asc" : "sort-desc");
      renderTable(summaries);
    });
  });
}

function buildRow(s: StockSummary): HTMLTableRowElement {
  const tr = document.createElement("tr");

  const cells: Partial<Record<ColKey, { text: string; detail?: string; cls?: string }>> = {
    ticker: { text: s.ticker },
    nome: { text: s.display_name ?? "—" },
    preco: { text: s.current_price != null ? `R$ ${fmt(s.current_price, 2)}` : "—" },
    liquidez: {
      text: s.liquidez_diaria != null ? `R$ ${fmtK(s.liquidez_diaria)}` : "—",
      detail: s.details.liquidez,
      cls: s.liquidez_diaria != null && s.liquidez_diaria < 200_000 ? "warn" : undefined,
    },
    pvp_atual: { text: s.pvp_atual != null ? fmt(s.pvp_atual, 2) : "—", detail: s.details.pvp_atual },
    pvp_medio_1a: { text: s.pvp_medio_1a != null ? fmt(s.pvp_medio_1a, 2) : "—", detail: s.details.pvp_medio_1a },
    pvp_medio_5a: { text: s.pvp_medio_5a != null ? fmt(s.pvp_medio_5a, 2) : "—", detail: s.details.pvp_medio_5a },
    pvp_justo: { text: s.pvp_justo != null ? fmt(s.pvp_justo, 2) : "N/A", detail: s.details.pvp_justo },
    pvp_justo_5a: { text: s.pvp_justo_5a != null ? fmt(s.pvp_justo_5a, 2) : "N/A", detail: s.details.pvp_justo_5a },
    roe_atual: { text: s.roe_atual != null ? `${fmt(s.roe_atual * 100, 1)}%` : "—", detail: s.details.roe_atual },
    roe_medio_5a: { text: s.roe_medio_5a != null ? `${fmt(s.roe_medio_5a * 100, 1)}%` : "—", detail: s.details.roe_medio_5a },
    dy_atual: { text: s.dy_atual != null ? `${fmt(s.dy_atual * 100, 1)}%` : "—", detail: s.details.dy_atual },
    dy_medio_5a: { text: s.dy_medio_5a != null ? `${fmt(s.dy_medio_5a * 100, 1)}%` : "—", detail: s.details.dy_medio_5a },
    g_usado: {
      text: s.g_usado != null
        ? `${fmt(s.g_usado * 100, 1)}% (${gFonteLabel(s.g_fonte)})`
        : "—",
      detail: s.details.g_usado,
    },
    vp_por_acao: { text: s.vp_por_acao != null ? `R$ ${fmt(s.vp_por_acao, 2)}` : "—" },
    net_income_ttm: { text: s.net_income_ttm != null ? `R$ ${fmtM(s.net_income_ttm)}` : "—" },
    dividends_ttm: { text: s.dividends_ttm != null ? `R$ ${fmt(s.dividends_ttm, 4)}` : "—" },
    // Novas métricas
    receita_ttm: {
      text: s.receita_ttm != null ? `R$ ${fmtM(s.receita_ttm)}` : "—",
      detail: s.details.receita_ttm,
    },
    margem_liquida: {
      text: s.margem_liquida != null ? `${fmt(s.margem_liquida * 100, 1)}%` : "—",
      detail: s.details.margem_liquida,
    },
    margem_operacional: {
      text: s.margem_operacional != null ? `${fmt(s.margem_operacional * 100, 1)}%` : "—",
      detail: s.details.margem_operacional,
    },
    roic: {
      text: s.roic != null ? `${fmt(s.roic * 100, 1)}%` : "—",
      detail: s.details.roic,
    },
    net_debt: {
      text: s.net_debt != null ? `R$ ${fmtM(s.net_debt)}` : "—",
      detail: s.details.net_debt,
    },
    divida_liquida_pl: {
      text: s.divida_liquida_pl != null ? `${fmt(s.divida_liquida_pl, 2)}x` : "—",
      detail: s.details.divida_liquida_pl,
      cls: s.divida_liquida_pl != null && s.divida_liquida_pl > 3 ? "warn" : undefined,
    },
    divida_liquida_ebit: {
      text: s.divida_liquida_ebit != null ? `${fmt(s.divida_liquida_ebit, 2)}x` : "—",
      detail: s.details.divida_liquida_ebit,
      cls: s.divida_liquida_ebit != null && s.divida_liquida_ebit > 3 ? "warn" : undefined,
    },
    pl: {
      text: s.pl != null ? `${fmt(s.pl, 1)}x` : "—",
      detail: s.details.pl,
    },
    ev_ebit: {
      text: s.ev_ebit != null ? `${fmt(s.ev_ebit, 1)}x` : "—",
      detail: s.details.ev_ebit,
    },
    flags: {
      text: (s.flags ?? []).join(" "),
      cls: (s.flags ?? []).length > 0 ? "has-flags" : undefined,
    },
    situacao_atual: {
      text: badgeText(s.situacao_atual, s.situacao_atual_reason),
      cls: situacaoClass(s.situacao_atual),
    },
    situacao_5a: {
      text: badgeText(s.situacao_5a, s.situacao_5a_reason),
      cls: situacaoClass(s.situacao_5a),
    },
    atualizado: { text: s.last_fetched_at ? tsToDate(s.last_fetched_at) : "—" },
  };

  for (const col of ALL_COLUMNS) {
    const td = document.createElement("td");
    td.dataset.col = col;
    const cell = cells[col];
    if (cell) {
      td.textContent = cell.text;
      if (cell.cls) td.classList.add(cell.cls);
      if (cell.detail) {
        td.dataset.detail = cell.detail;
        td.classList.add("has-detail");
      }
    }
    tr.appendChild(td);
  }

  return tr;
}

function gFonteLabel(fonte?: string): string {
  if (fonte === "fallback") return "fallback";
  if (fonte === "cagr_limitado") return "CAGR limitado";
  return "CAGR";
}

function badgeText(situacao: string, reason: string): string {
  const icons: Record<string, string> = {
    COMPRA: "🟢",
    MANTER: "🟡",
    VENDA: "🔴",
    DADOS_INSUFICIENTES: "⚠️",
  };
  const icon = icons[situacao] ?? "⚠️";
  return reason ? `${icon} ${situacao} (${reason})` : `${icon} ${situacao}`;
}

function situacaoClass(s: string): string {
  return { COMPRA: "badge-compra", MANTER: "badge-manter", VENDA: "badge-venda" }[s] ?? "badge-insuf";
}

function setStatus(msg: string, error = false) {
  const el = document.getElementById("status-msg")!;
  el.textContent = msg;
  el.className = "status-msg" + (error ? " error" : "");
}

// ─── Tooltips nos cabeçalhos das colunas ──────────────────────────────────────

const COL_FORMULAS: Partial<Record<ColKey, string>> = {
  pvp_atual: "P/VP = Preço / (Patrimônio Líquido / Ações)",
  pvp_justo: "(ROE TTM - g) / (r - g)",
  pvp_justo_5a: "(ROE médio 5a - g) / (r - g)",
  roe_atual: "ROE = Lucro Líquido TTM / Média(PL TTM)",
  dy_atual: "DY = Dividendos 12m / Preço Atual",
  liquidez: "Média(Fechamento × Volume, 252 pregões)",
  g_usado: "CAGR = (Lucro_final / Lucro_inicial)^(1/n) - 1",
  receita_ttm: "Σ(Receita, 4 trimestres)",
  margem_liquida: "Lucro Líquido TTM / Receita TTM",
  margem_operacional: "EBIT TTM / Receita TTM",
  roic: "NOPAT / (PL + Dívida LP - Caixa), NOPAT = EBIT × 0,66",
  net_debt: "Dívida LP - Caixa",
  divida_liquida_pl: "Dívida Líquida / Patrimônio Líquido",
  divida_liquida_ebit: "Dívida Líquida / EBIT TTM",
  pl: "Preço / (Lucro Líquido TTM / Ações)",
  ev_ebit: "(Market Cap + Dívida Líquida) / EBIT TTM",
};

function setupHeaderTooltips() {
  document.querySelectorAll<HTMLElement>("#analysis-table th[data-col]").forEach((th) => {
    const col = th.dataset.col as ColKey;
    const descricao = COL_DESCRIPTIONS[col];
    if (!descricao) return;
    const payload: Record<string, string> = { descricao };
    const formula = COL_FORMULAS[col];
    if (formula) payload.formula = formula;
    th.dataset.detail = JSON.stringify(payload);
    th.classList.add("has-detail");
    th.style.cursor = "help";
  });
}

// ─── Colunas (visibilidade) ────────────────────────────────────────────────────

function loadColumnPrefs() {
  try {
    const raw = localStorage.getItem(PREF_KEY);
    visibleCols = raw ? new Set(JSON.parse(raw) as ColKey[]) : new Set(DEFAULT_VISIBLE);
  } catch {
    visibleCols = new Set(DEFAULT_VISIBLE);
  }
}

function saveColumnPrefs() {
  localStorage.setItem(PREF_KEY, JSON.stringify([...visibleCols]));
}

function initColumnToggles() {
  const container = document.getElementById("column-toggles")!;
  container.innerHTML = "";

  // Botão que abre/fecha o painel
  const wrap = document.createElement("div");
  wrap.className = "col-dropdown-wrap";

  const trigger = document.createElement("button");
  trigger.className = "col-dropdown-trigger";
  trigger.type = "button";
  trigger.textContent = `Colunas ▾`;

  const panel = document.createElement("div");
  panel.className = "col-dropdown-panel";
  panel.style.display = "none";

  const updateTriggerLabel = () => {
    const n = ALL_COLUMNS.filter(c => c !== "ticker" && visibleCols.has(c)).length;
    trigger.textContent = `Colunas (${n}) ▾`;
  };
  updateTriggerLabel();

  // Preenche o painel com as pills
  for (const col of ALL_COLUMNS) {
    if (col === "ticker") continue;
    const pill = document.createElement("button");
    pill.type = "button";
    pill.className = "col-toggle-btn" + (visibleCols.has(col) ? " active" : "");
    pill.textContent = COL_LABELS[col];
    pill.addEventListener("click", () => {
      if (visibleCols.has(col)) {
        visibleCols.delete(col);
        pill.classList.remove("active");
      } else {
        visibleCols.add(col);
        pill.classList.add("active");
      }
      saveColumnPrefs();
      applyColumnVisibility();
      updateTriggerLabel();
    });
    panel.appendChild(pill);
  }

  trigger.addEventListener("click", (e) => {
    e.stopPropagation();
    const open = panel.style.display !== "none";
    panel.style.display = open ? "none" : "flex";
    trigger.classList.toggle("open", !open);
  });

  // Fecha ao clicar fora
  document.addEventListener("click", (e) => {
    if (!wrap.contains(e.target as Node)) {
      panel.style.display = "none";
      trigger.classList.remove("open");
    }
  });

  wrap.appendChild(trigger);
  wrap.appendChild(panel);
  container.appendChild(wrap);
}

function applyColumnVisibility() {
  document.querySelectorAll<HTMLElement>("[data-col]").forEach((el) => {
    const col = el.dataset.col as ColKey;
    const visible = col === "ticker" || visibleCols.has(col);
    el.style.display = visible ? "" : "none";
  });
}

// ─── Aba Ativos ───────────────────────────────────────────────────────────────

function setupWatchlistTab() {
  const input = document.getElementById("ticker-input") as HTMLInputElement;
  const btn = document.getElementById("btn-add-ticker") as HTMLButtonElement;
  const status = document.getElementById("add-status")!;

  const doAdd = async () => {
    const ticker = input.value.trim().toUpperCase();
    if (!ticker) return;
    btn.disabled = true;
    status.textContent = `Buscando dados de ${ticker}…`;
    status.className = "add-status";
    try {
      const summary = await invoke<StockSummary>("add_ticker", { ticker });
      input.value = "";
      const warnings = summary.warnings ?? [];
      status.textContent = warnings.length
        ? `${ticker} adicionado, mas: ${warnings.join(" | ")}`
        : `${ticker} adicionado com sucesso!`;
      status.className = warnings.length ? "add-status warn" : "add-status ok";
      await loadWatchlist();
      await loadAnalysis();
    } catch (e) {
      status.textContent = `Erro: ${e}`;
      status.className = "add-status error";
    } finally {
      btn.disabled = false;
    }
  };

  btn.addEventListener("click", doAdd);
  input.addEventListener("keydown", (e) => { if (e.key === "Enter") doAdd(); });
}

async function loadWatchlist() {
  const container = document.getElementById("watchlist-container")!;
  try {
    const list = await invoke<WatchlistEntry[]>("get_watchlist");
    container.innerHTML = "";

    if (list.length === 0) {
      container.innerHTML = '<p class="empty-msg">Nenhum ativo cadastrado ainda.</p>';
      return;
    }

    for (const entry of list) {
      container.appendChild(buildWatchlistRow(entry));
    }
  } catch (e) {
    container.innerHTML = `<p class="error">Erro: ${e}</p>`;
  }
}

function buildWatchlistRow(entry: WatchlistEntry): HTMLElement {
  const div = document.createElement("div");
  div.className = "watchlist-row";
  div.innerHTML = `
    <div class="wl-info">
      <span class="wl-ticker">${entry.ticker}</span>
      <span class="wl-name">${entry.display_name ?? ""}</span>
      <span class="wl-date">Atualizado: ${entry.last_fetched_at ? tsToDate(entry.last_fetched_at) : "nunca"}</span>
    </div>
    <div class="wl-actions">
      <button class="btn-refresh" data-ticker="${entry.ticker}">↻ Atualizar</button>
      <button class="btn-remove" data-ticker="${entry.ticker}">✕ Remover</button>
    </div>
  `;

  div.querySelector(".btn-refresh")!.addEventListener("click", async (e) => {
    const btn = e.currentTarget as HTMLButtonElement;
    const ticker = btn.dataset.ticker!;
    btn.disabled = true;
    btn.textContent = "Buscando…";
    try {
      const summary = await invoke<StockSummary>("refresh_ticker", { ticker });
      await loadWatchlist();
      await loadAnalysis();
      showRowWarnings(ticker, summary.warnings ?? []);
    } catch (err) {
      alert(`Erro ao atualizar ${ticker}: ${err}`);
    } finally {
      btn.disabled = false;
      btn.textContent = "↻ Atualizar";
    }
  });

  div.querySelector(".btn-remove")!.addEventListener("click", async (e) => {
    const btn = e.currentTarget as HTMLButtonElement;
    const ticker = btn.dataset.ticker!;
    if (!confirm(`Remover ${ticker} e todos os seus dados do cache?`)) return;
    try {
      await invoke("remove_ticker", { ticker });
      await loadWatchlist();
      await loadAnalysis();
    } catch (err) {
      alert(`Erro ao remover ${ticker}: ${err}`);
    }
  });

  return div;
}

/** Mostra falhas parciais da coleta na linha do ativo (a lista é reconstruída a cada refresh) */
function showRowWarnings(ticker: string, warnings: string[]) {
  if (warnings.length === 0) return;
  const btn = document.querySelector<HTMLElement>(`.btn-refresh[data-ticker="${ticker}"]`);
  const row = btn?.closest(".watchlist-row");
  if (!row) return;
  const box = document.createElement("div");
  box.className = "wl-warn";
  box.textContent = `⚠️ ${warnings.join(" | ")}`;
  row.appendChild(box);
}

// ─── Aba Configurações ────────────────────────────────────────────────────────

async function loadSettings() {
  try {
    const s = await invoke<AppSettings>("get_settings");
    (document.getElementById("cfg-r") as HTMLInputElement).value = (s.desired_return_rate * 100).toFixed(1);
    (document.getElementById("cfg-g") as HTMLInputElement).value = (s.fallback_growth_rate * 100).toFixed(1);
    (document.getElementById("cfg-gmax") as HTMLInputElement).value = (s.max_growth_rate * 100).toFixed(1);
    (document.getElementById("cfg-dymin") as HTMLInputElement).value = (s.min_dividend_yield * 100).toFixed(1);
    (document.getElementById("cfg-liq") as HTMLInputElement).value = s.min_daily_liquidity.toFixed(0);
    (document.getElementById("cfg-selic") as HTMLInputElement).value = (s.selic_rate * 100).toFixed(2);
  } catch (e) {
    console.error("Erro ao carregar configurações:", e);
  }
}

function setupSettingsForm() {
  const form = document.getElementById("settings-form")!;
  const status = document.getElementById("settings-status")!;

  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const settings: AppSettings = {
      desired_return_rate: parseFloat((document.getElementById("cfg-r") as HTMLInputElement).value) / 100,
      fallback_growth_rate: parseFloat((document.getElementById("cfg-g") as HTMLInputElement).value) / 100,
      max_growth_rate: parseFloat((document.getElementById("cfg-gmax") as HTMLInputElement).value) / 100,
      min_dividend_yield: parseFloat((document.getElementById("cfg-dymin") as HTMLInputElement).value) / 100,
      min_daily_liquidity: parseFloat((document.getElementById("cfg-liq") as HTMLInputElement).value),
      selic_rate: parseFloat((document.getElementById("cfg-selic") as HTMLInputElement).value) / 100,
    };

    status.textContent = "Salvando e recalculando…";
    status.className = "settings-status";
    try {
      summaries = await invoke<StockSummary[]>("save_settings", { settings });
      renderTable(summaries);
      status.textContent = "Salvo! Métricas recalculadas.";
      status.className = "settings-status ok";
    } catch (err) {
      status.textContent = `Erro: ${err}`;
      status.className = "settings-status error";
    }
  });
}

// ─── Tooltip ──────────────────────────────────────────────────────────────────

function setupTooltip() {
  const box = document.getElementById("tooltip-box")!;

  document.addEventListener("mouseover", (e) => {
    const td = (e.target as HTMLElement).closest("[data-detail]") as HTMLElement | null;
    if (!td) return;
    const raw = td.dataset.detail;
    if (!raw) return;
    box.innerHTML = formatDetail(raw);
    box.style.display = "block";
    positionTooltip(e, box);
  });

  document.addEventListener("mousemove", (e) => {
    if (box.style.display === "none") return;
    positionTooltip(e, box);
  });

  document.addEventListener("mouseout", (e) => {
    const td = (e.target as HTMLElement).closest("[data-detail]");
    if (td) box.style.display = "none";
  });
}

function positionTooltip(e: MouseEvent, box: HTMLElement) {
  const margin = 12;
  let left = e.clientX + margin;
  let top = e.clientY + margin;
  if (left + box.offsetWidth > window.innerWidth - 8) {
    left = e.clientX - box.offsetWidth - margin;
  }
  if (top + box.offsetHeight > window.innerHeight - 8) {
    top = e.clientY - box.offsetHeight - margin;
  }
  box.style.left = `${left}px`;
  box.style.top = `${top}px`;
}

function formatDetail(raw: string): string {
  try {
    const obj = JSON.parse(raw);
    const lines: string[] = [];
    for (const [k, v] of Object.entries(obj)) {
      if (Array.isArray(v)) {
        lines.push(`<b>${k}</b>:`);
        for (const item of v) {
          if (typeof item === "object") {
            lines.push("  " + Object.entries(item).map(([ik, iv]) => `${ik}: ${iv}`).join(" | "));
          } else {
            lines.push(`  ${item}`);
          }
        }
      } else {
        lines.push(`<b>${k}</b>: ${v}`);
      }
    }
    return lines.join("<br>");
  } catch {
    return raw;
  }
}

// ─── Formatação ───────────────────────────────────────────────────────────────

function fmt(v: number, dec: number): string {
  return v.toLocaleString("pt-BR", { minimumFractionDigits: dec, maximumFractionDigits: dec });
}

function fmtK(v: number): string {
  if (v >= 1_000_000) return `${fmt(v / 1_000_000, 1)}M`;
  if (v >= 1_000) return `${fmt(v / 1_000, 0)}K`;
  return fmt(v, 0);
}

/** Formata valores monetários grandes (lucro líquido TTM etc.) */
function fmtM(v: number): string {
  const abs = Math.abs(v);
  const sign = v < 0 ? "-" : "";
  if (abs >= 1_000_000_000) return `${sign}${fmt(abs / 1_000_000_000, 2)}B`;
  if (abs >= 1_000_000) return `${sign}${fmt(abs / 1_000_000, 1)}M`;
  if (abs >= 1_000) return `${sign}${fmt(abs / 1_000, 0)}K`;
  return `${sign}${fmt(abs, 0)}`;
}

function tsToDate(ts: number): string {
  return new Date(ts * 1000).toLocaleDateString("pt-BR");
}
