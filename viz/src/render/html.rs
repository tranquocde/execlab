//! Monospace, bordered HTML renderer for the execution sweep view.

use crate::{process::Report, render::Renderer};

pub struct HtmlRenderer;

impl Renderer for HtmlRenderer {
    fn render(&self, _report: &Report) -> Result<String, String> {
        let mut html = HEAD.replace(
            "<script id=\"report-data\" type=\"application/json\">",
            "<script>",
        );
        html.push_str(LAZY_CLIENT);
        Ok(html)
    }
}

const HEAD: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Execution Backtest Dashboard</title>
  <style>
    * { box-sizing: border-box; }
    body {
      margin: 0; min-height: 100vh; padding: 16px; background: #f3f3f3;
      color: #111; font-family: "Courier New", Courier, monospace;
    }
    .dashboard {
      width: min(1900px, 100%); margin: 0 auto; display: grid;
      grid-template-columns: minmax(0, 1fr) minmax(500px, 620px);
      background: #f7f7f7; border: 2px solid #222; font-size: 16px;
      line-height: 1.4;
    }
    .dashboard-main { min-width: 0; }
    .section { padding: 16px 20px; border-bottom: 2px solid #222; overflow-x: hidden; }
    .section:last-child { border-bottom: 0; }
    .top-title, .panel-title {
      display: flex; align-items: center; justify-content: space-between;
      gap: 20px; font-weight: 700;
    }
    .top-title { justify-content: flex-start; font-size: 24px; margin-bottom: 16px; }
    .brand { margin-left: auto; color: #6f2da8; font-size: 31px; font-weight: 700; letter-spacing: .4px; }
    .title-icon {
      display: inline-grid; place-items: center; width: 32px; height: 32px;
      border: 2px solid #111;
    }
    .panel-title { margin-bottom: 12px; }
    .controls, .interval-head { display: flex; gap: 24px; flex-wrap: wrap; }
    .interval-head { justify-content: space-between; margin-bottom: 12px; }
    .pager { display: flex; align-items: center; gap: 8px; margin: 8px 0 12px; }
    .pager button, .pager select {
      border: 1px solid #222; background: #f7f7f7; color: #111;
      font: inherit; padding: 2px 7px;
    }
    .pager input {
      width: 58px; border: 1px solid #222; background: #fff; color: #111;
      font: inherit; padding: 2px 6px; text-align: right;
    }
    .pager button { cursor: pointer; }
    .expand-button { border: 1px solid #222; background: #f7f7f7; color: #111;
      font: inherit; font-weight: 700; padding: 3px 10px; cursor: pointer; }
    .replay-link { margin-left: 10px; border: 1px solid #6f2da8; background: #6f2da8;
      color: #fff; font: inherit; font-weight: 700; padding: 1px 7px; cursor: pointer; }
    .pager button:disabled { color: #999; cursor: default; }
    .table-scroll { width: 100%; overflow-x: auto; scrollbar-gutter: stable; }
    table { width: max-content; min-width: 100%; border-collapse: collapse; }
    th, td { padding: 4px 18px 4px 0; text-align: left; white-space: nowrap; }
    th { border-bottom: 1px dashed #777; }
    th[data-sort] { cursor: pointer; user-select: none; }
    th.sort-asc::after { content: " ▲"; }
    th.sort-desc::after { content: " ▼"; }
    th:not(:first-child), td:not(:first-child) { text-align: right; }
    #strategy-table th:first-child, #strategy-table td:first-child,
    #asset-table th:first-child, #asset-table td:first-child,
    #interval-table th:first-child, #interval-table td:first-child { text-align: right; }
    #strategy-table th:nth-child(2), #strategy-table td:nth-child(2),
    #asset-table th:nth-child(2), #asset-table td:nth-child(2),
    #interval-table th:nth-child(2), #interval-table td:nth-child(2) { text-align: left; }
    tr[data-index] { cursor: pointer; }
    tr[data-index]:hover td { background: #e6e6e6; }
    tr.selected td { font-weight: 700; background: #dedede; }
    .status-ok { color: #14833b; font-weight: 700; }
    .status-fail { color: #b00020; font-weight: 700; }
    .cost-good { color: #14833b; font-weight: 700; }
    .cost-bad { color: #b00020; font-weight: 700; }
    .provenance { display: block; max-width: 520px; overflow: hidden; text-overflow: ellipsis;
      color: #555; font-size: 12px; font-weight: 400; }
    .template-panel { border-left: 2px solid #222; min-width: 0; }
    .template-panel .section:last-child { border-bottom: 0; }
    .config-table { width: 100%; min-width: 0; table-layout: fixed; }
    .config-table th, .config-table td { white-space: normal; overflow-wrap: anywhere; }
    .config-table th:first-child, .config-table td:first-child { width: 48%; }
    .config-table tbody td { border-bottom: 1px dashed #bbb; }
    .config-table tbody tr:last-child td { border-bottom: 0; }
    .config-table tr.config-highlight td { font-weight: 700; }
    pre {
      margin: 0; padding-top: 12px; border-top: 2px dashed #8f8f8f;
      font: 13px/1.35 "Courier New", Courier, monospace; white-space: pre-wrap;
      overflow-wrap: anywhere; max-height: calc(100vh - 180px); overflow-y: auto;
      scrollbar-gutter: stable;
    }
    .warning { padding: 8px 20px; border-bottom: 2px solid #222; color: #700; }
    .empty { color: #666; }
    @media (max-width: 1150px) {
      .dashboard { grid-template-columns: 1fr; }
      .template-panel { border-left: 0; border-top: 2px solid #222; }
    }
  </style>
</head>
<body>
  <main class="dashboard">
    <div class="dashboard-main">
      <section class="section">
        <div class="top-title"><span class="title-icon">▦</span><span>EXECUTION BACKTEST DASHBOARD</span><span class="brand">VPS Securities JSC</span></div>
        <div class="controls">
          <span id="strategy-count"></span><span id="asset-count"></span><span id="interval-count"></span>
        </div>
      </section>

      <div id="warnings"></div>

      <section class="section">
        <div class="panel-title"><span>STRATEGY COMPARISON</span><span>click a row to drill down</span></div>
        <div class="pager">
          <span>Rows:</span><select id="strategy-size"><option>5</option><option>10</option><option>20</option><option>50</option></select>
          <button id="strategy-prev">&lt;&lt;&lt;</button><input id="strategy-page" type="number" min="1" value="1"><span id="strategy-pages"></span><button id="strategy-next">&gt;&gt;&gt;</button>
        </div>
        <div class="table-scroll"><table id="strategy-table">
          <thead><tr><th>#</th><th data-sort="strategy">Strategy</th><th data-sort="mean_cost">Mean IS %</th><th data-sort="p15_cost">P15 IS %</th><th data-sort="p50_cost">P50 IS %</th><th data-sort="p90_cost">P90 IS %</th><th data-sort="avg_mean_divergence">Avg Mean Divergence</th><th data-sort="avg_max_divergence">Avg Max Divergence</th><th data-sort="completion">Completion</th><th data-sort="fees">Fees</th><th data-sort="status">Status</th></tr></thead>
          <tbody id="strategy-body"></tbody>
        </table></div>
      </section>

      <section class="section">
        <div class="panel-title"><span>ASSET COMPARISON</span><span><span id="selected-strategy"></span> <button id="asset-expand" class="expand-button">Expand</button></span></div>
        <div class="pager">
          <span>Rows:</span><select id="asset-size"><option>5</option><option>10</option><option>20</option><option>50</option></select>
          <button id="asset-prev">&lt;&lt;&lt;</button><input id="asset-page" type="number" min="1" value="1"><span id="asset-pages"></span><button id="asset-next">&gt;&gt;&gt;</button>
        </div>
        <div class="table-scroll"><table id="asset-table">
          <thead><tr><th>#</th><th data-sort="asset">Asset</th><th data-sort="runs">Runs</th><th data-sort="mean_cost">Mean IS %</th><th data-sort="p15_cost">P15 IS %</th><th data-sort="p50_cost">P50 IS %</th><th data-sort="p90_cost">P90 IS %</th><th data-sort="avg_mean_divergence">Avg Mean Divergence</th><th data-sort="avg_max_divergence">Avg Max Divergence</th><th data-sort="completion">Completion</th><th data-sort="fees">Fees</th><th data-sort="status">Status</th></tr></thead>
          <tbody id="asset-body"></tbody>
        </table></div>
      </section>

      <section class="section">
        <div class="panel-title"><span>INTERVAL EXPLORER</span><span>single-session sweep results</span></div>
        <div class="interval-head"><span id="interval-strategy"></span><span id="interval-asset"></span></div>
        <div class="pager">
          <span>Rows:</span><select id="interval-size"><option>5</option><option>10</option><option>20</option><option>50</option></select>
          <button id="interval-prev">&lt;&lt;&lt;</button><input id="interval-page" type="number" min="1" value="1"><span id="interval-pages"></span><button id="interval-next">&gt;&gt;&gt;</button>
        </div>
        <div class="table-scroll"><table id="interval-table">
          <thead><tr><th>#</th><th data-sort="interval">Interval</th><th data-sort="start_position">Initial Position</th><th data-sort="final_balance">Final Balance</th><th data-sort="num_orders">Num Orders</th><th data-sort="num_trades">Num Trades</th><th data-sort="n_maker">N Maker</th><th data-sort="avg_filled_price">Avg Filled Price</th><th data-sort="percent_filled">% Filled</th><th data-sort="cost">Implementation Shortfall %</th><th data-sort="mean_divergence">Mean Divergence Score</th><th data-sort="max_divergence">Max Divergence Score</th><th data-sort="completion">Completion</th><th data-sort="fees">Fees</th><th data-sort="status">Status</th></tr></thead>
          <tbody id="interval-body"></tbody>
        </table></div>
      </section>
    </div>

    <aside class="template-panel">
      <section class="section">
        <div class="panel-title"><span>BACKTEST CONFIG</span></div>
        <table class="config-table">
          <thead><tr><th>Configuration</th><th>Value</th></tr></thead>
          <tbody id="backtest-config-body"></tbody>
        </table>
      </section>
      <section class="section">
        <div class="panel-title"><span>STRATEGY TEMPLATE</span><span id="template-strategy"></span></div>
        <div id="template-params"></div>
        <pre id="strategy-source"></pre>
      </section>
    </aside>
  </main>

  <script id="report-data" type="application/json">"#;

const _TAIL: &str = r#"</script>
  <script>
    const report = JSON.parse(document.getElementById('report-data').textContent);
    let selectedStrategy = 0;
    let selectedAsset = 0;
    const page = { strategy: 0, asset: 0, interval: 0 };
    const pageSize = { strategy: 5, asset: 5, interval: 5 };

    const byId = id => document.getElementById(id);
    const number = value => value == null || !Number.isFinite(value) ? '—' : value.toFixed(4);
    const percent = value => value == null || !Number.isFinite(value) ? '—' : value.toFixed(2) + '%';
    const money = value => value == null || !Number.isFinite(value) ? '—' : value.toFixed(4);
    const strategyName = strategy => `${strategy.alpha} / ${strategy.hash}`;
    const assetName = asset => `${asset.timeframe}/${asset.asset.toUpperCase()}`;
    const costClass = value => value < 0 ? 'cost-good' : value > 0 ? 'cost-bad' : '';

    function pageItems(items, key) {
      const pageCount = Math.max(1, Math.ceil(items.length / pageSize[key]));
      page[key] = Math.min(page[key], pageCount - 1);
      const start = page[key] * pageSize[key];
      const input = byId(`${key}-page`);
      input.max = pageCount;
      input.value = page[key] + 1;
      byId(`${key}-pages`).textContent = `/ ${pageCount}`;
      byId(`${key}-prev`).disabled = page[key] === 0;
      byId(`${key}-next`).disabled = page[key] >= pageCount - 1;
      return items.slice(start, start + pageSize[key]).map((item, offset) => [item, start + offset]);
    }

    function cell(row, value, className = '') {
      const td = document.createElement('td');
      td.textContent = value;
      if (className) td.className = className;
      row.appendChild(td);
      return td;
    }

    function renderStrategies() {
      const body = byId('strategy-body');
      body.replaceChildren();
      pageItems(report.strategies, 'strategy').forEach(([strategy, index]) => {
        const row = document.createElement('tr');
        row.dataset.index = index;
        if (index === selectedStrategy) row.className = 'selected';
        const name = cell(row, strategyName(strategy));
        const provenance = document.createElement('span');
        provenance.className = 'provenance';
        provenance.textContent = strategy.source_path;
        name.appendChild(provenance);
        cell(row, percent(strategy.stats.mean_cost), costClass(strategy.stats.mean_cost));
        cell(row, percent(strategy.stats.p15_cost), costClass(strategy.stats.p15_cost));
        cell(row, percent(strategy.stats.p50_cost), costClass(strategy.stats.p50_cost));
        cell(row, percent(strategy.stats.p90_cost), costClass(strategy.stats.p90_cost));
        cell(row, number(strategy.stats.avg_mean_divergence));
        cell(row, number(strategy.stats.avg_max_divergence));
        cell(row, percent(strategy.stats.completion_pct));
        cell(row, money(strategy.stats.fees));
        cell(
          row,
          percent(strategy.stats.status_pct),
          strategy.stats.status_pct > 95 ? 'status-ok' : 'status-fail'
        );
        row.addEventListener('click', () => {
          selectedStrategy = index; selectedAsset = 0;
          page.asset = 0; page.interval = 0; renderAll();
        });
        body.appendChild(row);
      });
    }

    function renderAssets() {
      const strategy = report.strategies[selectedStrategy];
      const body = byId('asset-body');
      body.replaceChildren();
      byId('selected-strategy').textContent = strategyName(strategy);
      pageItems(strategy.assets, 'asset').forEach(([asset, index]) => {
        const row = document.createElement('tr');
        row.dataset.index = index;
        if (index === selectedAsset) row.className = 'selected';
        cell(row, assetName(asset));
        cell(row, String(asset.stats.runs));
        cell(row, percent(asset.stats.mean_cost), costClass(asset.stats.mean_cost));
        cell(row, percent(asset.stats.p15_cost), costClass(asset.stats.p15_cost));
        cell(row, percent(asset.stats.p50_cost), costClass(asset.stats.p50_cost));
        cell(row, percent(asset.stats.p90_cost), costClass(asset.stats.p90_cost));
        cell(row, number(asset.stats.avg_mean_divergence));
        cell(row, number(asset.stats.avg_max_divergence));
        cell(row, percent(asset.stats.completion_pct));
        cell(row, money(asset.stats.fees));
        cell(
          row,
          percent(asset.stats.status_pct),
          asset.stats.status_pct > 95 ? 'status-ok' : 'status-fail'
        );
        row.addEventListener('click', () => {
          selectedAsset = index; page.interval = 0; renderAll();
        });
        body.appendChild(row);
      });
    }

    function renderIntervals() {
      const strategy = report.strategies[selectedStrategy];
      const asset = strategy.assets[selectedAsset];
      const body = byId('interval-body');
      body.replaceChildren();
      byId('interval-strategy').textContent = `Strategy: ${strategyName(strategy)}`;
      byId('interval-asset').textContent = asset ? `Asset: ${assetName(asset)}` : 'Asset: —';
      if (!asset) return;
      pageItems(asset.intervals, 'interval').forEach(([interval]) => {
        const row = document.createElement('tr');
        const intervalCell = cell(row, interval.session_id);
        const replay = document.createElement('button'); replay.className = 'replay-link'; replay.textContent = 'Replay';
        replay.onclick = () => window.open(`/session-replay?strategy=${selectedStrategy}&asset=${selectedAsset}&session=${encodeURIComponent(interval.session_id)}`, '_blank');
        intervalCell.appendChild(replay);
        cell(row, number(interval.start_position));
        cell(row, money(interval.final_balance));
        cell(row, String(interval.num_orders));
        cell(row, String(interval.num_trades));
        cell(row, String(interval.n_maker));
        cell(row, number(interval.avg_filled_price));
        cell(row, percent(interval.percent_filled));
        cell(row, percent(interval.cost), costClass(interval.cost));
        cell(row, number(interval.mean_divergence_score));
        cell(row, number(interval.max_divergence_score));
        cell(row, percent(interval.completion_pct));
        cell(row, money(interval.fees));
        cell(row, interval.status_ok ? 'OK' : 'FAIL', interval.status_ok ? 'status-ok' : 'status-fail');
        body.appendChild(row);
      });
    }

    function renderTemplate() {
      const strategy = report.strategies[selectedStrategy];
      byId('template-strategy').textContent = strategyName(strategy);
      byId('template-params').textContent = `Params: ${JSON.stringify(strategy.params)}`;
      byId('strategy-source').textContent = strategy.strategy_source || 'strategy.rs unavailable';
    }

    function renderWarnings() {
      const root = byId('warnings'); root.replaceChildren();
      report.warnings.forEach(warning => {
        const item = document.createElement('div'); item.className = 'warning';
        item.textContent = warning; root.appendChild(item);
      });
    }

    function renderAll() {
      const strategies = report.strategies;
      const assets = strategies.reduce((sum, strategy) => sum + strategy.assets.length, 0);
      const intervals = strategies.reduce((sum, strategy) =>
        sum + strategy.assets.reduce((n, asset) => n + asset.intervals.length, 0), 0);
      byId('strategy-count').textContent = `Strategies: ${strategies.length}`;
      byId('asset-count').textContent = `Assets: ${assets}`;
      byId('interval-count').textContent = `Intervals: ${intervals}`;
      renderStrategies(); renderAssets(); renderIntervals(); renderTemplate(); renderWarnings();
    }

    function bindPager(key, render) {
      byId(`${key}-size`).addEventListener('change', event => {
        pageSize[key] = Number(event.target.value); page[key] = 0; render();
      });
      byId(`${key}-prev`).addEventListener('click', () => {
        if (page[key] > 0) page[key] -= 1; render();
      });
      byId(`${key}-next`).addEventListener('click', () => {
        page[key] += 1; render();
      });
      const jump = () => {
        const requested = Number(byId(`${key}-page`).value);
        if (Number.isFinite(requested)) page[key] = Math.max(0, Math.floor(requested) - 1);
        render();
      };
      byId(`${key}-page`).addEventListener('change', jump);
      byId(`${key}-page`).addEventListener('keydown', event => {
        if (event.key === 'Enter') { event.preventDefault(); jump(); }
      });
    }

    bindPager('strategy', renderStrategies);
    bindPager('asset', renderAssets);
    bindPager('interval', renderIntervals);
    renderAll();
  </script>
</body>
</html>
"#;

const LAZY_CLIENT: &str = r#"
    let selectedStrategy = 0;
    let selectedAsset = 0;
    const page = { strategy: 1, asset: 1, interval: 1 };
    const pageSize = { strategy: 5, asset: 5, interval: 5 };
    const sorting = {
      strategy: { column: 'strategy', order: 'asc' },
      asset: { column: 'asset', order: 'asc' },
      interval: { column: 'interval', order: 'asc' }
    };
    const responseCache = new Map();

    const byId = id => document.getElementById(id);
    const number = value => value == null || !Number.isFinite(value) ? '—' : value.toFixed(4);
    const percent = value => value == null || !Number.isFinite(value) ? '—' : value.toFixed(2) + '%';
    const money = value => value == null || !Number.isFinite(value) ? '—' : value.toFixed(4);
    const strategyName = strategy => `${strategy.alpha} / ${strategy.hash}`;
    const assetName = asset => `${asset.timeframe}/${asset.asset.toUpperCase()}`;
    const costClass = value => value < 0 ? 'cost-good' : value > 0 ? 'cost-bad' : '';
    const sortQuery = key => sorting[key].column
      ? `&sort=${sorting[key].column}&order=${sorting[key].order}` : '';

    async function fetchJson(url) {
      if (responseCache.has(url)) return responseCache.get(url);

      const pending = fetch(url).then(async response => {
        if (!response.ok) throw new Error(await response.text());
        return response.json();
      });
      responseCache.set(url, pending);

      try {
        return await pending;
      } catch (error) {
        responseCache.delete(url);
        throw error;
      }
    }

    function cell(row, value, className = '') {
      const td = document.createElement('td');
      td.textContent = value;
      if (className) td.className = className;
      row.appendChild(td);
      return td;
    }

    function updatePager(key, result) {
      page[key] = result.page;
      const input = byId(`${key}-page`);
      input.max = result.pages;
      input.value = result.page;
      byId(`${key}-pages`).textContent = `/ ${result.pages}`;
      byId(`${key}-prev`).disabled = result.page <= 1;
      byId(`${key}-next`).disabled = result.page >= result.pages;
    }

    async function loadMeta() {
      const meta = await fetchJson(`/api/meta?refresh=${Date.now()}`);
      byId('strategy-count').textContent = `Strategies: ${meta.strategies}`;
      byId('asset-count').textContent = `Assets: ${meta.assets}`;
      byId('interval-count').textContent = `Intervals: ${meta.intervals}`;
      const root = byId('warnings'); root.replaceChildren();
      meta.warnings.forEach(warning => {
        const item = document.createElement('div'); item.className = 'warning';
        item.textContent = warning; root.appendChild(item);
      });
      return meta;
    }

    async function loadStrategies() {
      const result = await fetchJson(`/api/strategies?page=${page.strategy}&size=${pageSize.strategy}${sortQuery('strategy')}`);
      updatePager('strategy', result);
      const body = byId('strategy-body'); body.replaceChildren();
      result.items.forEach((strategy, offset) => {
        const row = document.createElement('tr'); row.dataset.index = strategy.index;
        if (strategy.index === selectedStrategy) row.className = 'selected';
        cell(row, String((result.page - 1) * result.page_size + offset + 1));
        const name = cell(row, strategyName(strategy));
        const provenance = document.createElement('span'); provenance.className = 'provenance';
        provenance.textContent = strategy.source_path; name.appendChild(provenance);
        cell(row, percent(strategy.stats.mean_cost), costClass(strategy.stats.mean_cost));
        cell(row, percent(strategy.stats.p15_cost), costClass(strategy.stats.p15_cost));
        cell(row, percent(strategy.stats.p50_cost), costClass(strategy.stats.p50_cost));
        cell(row, percent(strategy.stats.p90_cost), costClass(strategy.stats.p90_cost));
        cell(row, number(strategy.stats.avg_mean_divergence));
        cell(row, number(strategy.stats.avg_max_divergence));
        cell(row, percent(strategy.stats.completion_pct));
        cell(row, money(strategy.stats.fees));
        cell(row, percent(strategy.stats.status_pct), strategy.stats.status_pct > 95 ? 'status-ok' : 'status-fail');
        row.addEventListener('click', async () => {
          selectedStrategy = strategy.index; selectedAsset = 0;
          page.asset = 1; page.interval = 1;
          await Promise.all([loadStrategies(), loadAssets(), loadSource()]);
        });
        body.appendChild(row);
      });
    }

    async function loadAssets() {
      const result = await fetchJson(`/api/assets?strategy=${selectedStrategy}&page=${page.asset}&size=${pageSize.asset}${sortQuery('asset')}`);
      updatePager('asset', result);
      const body = byId('asset-body'); body.replaceChildren();
      result.items.forEach((asset, offset) => {
        const row = document.createElement('tr'); row.dataset.index = asset.index;
        if (asset.index === selectedAsset) row.className = 'selected';
        cell(row, String((result.page - 1) * result.page_size + offset + 1));
        cell(row, assetName(asset));
        cell(row, String(asset.stats.runs));
        cell(row, percent(asset.stats.mean_cost), costClass(asset.stats.mean_cost));
        cell(row, percent(asset.stats.p15_cost), costClass(asset.stats.p15_cost));
        cell(row, percent(asset.stats.p50_cost), costClass(asset.stats.p50_cost));
        cell(row, percent(asset.stats.p90_cost), costClass(asset.stats.p90_cost));
        cell(row, number(asset.stats.avg_mean_divergence));
        cell(row, number(asset.stats.avg_max_divergence));
        cell(row, percent(asset.stats.completion_pct));
        cell(row, money(asset.stats.fees));
        cell(row, percent(asset.stats.status_pct), asset.stats.status_pct > 95 ? 'status-ok' : 'status-fail');
        row.addEventListener('click', async () => {
          selectedAsset = asset.index; page.interval = 1;
          await loadAssets();
        });
        body.appendChild(row);
      });
      const selected = result.items.find(asset => asset.index === selectedAsset);
      byId('interval-asset').textContent = selected ? `Asset: ${assetName(selected)}` : 'Asset: selected';
      await loadIntervals();
    }

    async function loadIntervals() {
      const result = await fetchJson(`/api/intervals?strategy=${selectedStrategy}&asset=${selectedAsset}&page=${page.interval}&size=${pageSize.interval}${sortQuery('interval')}`);
      updatePager('interval', result);
      const body = byId('interval-body'); body.replaceChildren();
      result.items.forEach((interval, offset) => {
        const row = document.createElement('tr');
        cell(row, String((result.page - 1) * result.page_size + offset + 1));
        const intervalCell = cell(row, interval.session_id);
        const replay = document.createElement('button'); replay.className = 'replay-link'; replay.textContent = 'Replay';
        replay.onclick = () => window.open(`/session-replay?strategy=${selectedStrategy}&asset=${selectedAsset}&session=${encodeURIComponent(interval.session_id)}`, '_blank');
        intervalCell.appendChild(replay);
        cell(row, number(interval.start_position));
        cell(row, money(interval.final_balance));
        cell(row, String(interval.num_orders));
        cell(row, String(interval.num_trades));
        cell(row, String(interval.n_maker));
        cell(row, number(interval.avg_filled_price));
        cell(row, percent(interval.percent_filled));
        cell(row, percent(interval.cost), costClass(interval.cost));
        cell(row, number(interval.mean_divergence_score));
        cell(row, number(interval.max_divergence_score));
        cell(row, percent(interval.completion_pct));
        cell(row, money(interval.fees));
        cell(row, interval.status_ok ? 'OK' : 'FAIL', interval.status_ok ? 'status-ok' : 'status-fail');
        body.appendChild(row);
      });
    }

    async function loadSource() {
      const strategy = await fetchJson(`/api/source?strategy=${selectedStrategy}`);
      byId('selected-strategy').textContent = strategy.name;
      byId('interval-strategy').textContent = `Strategy: ${strategy.name}`;
      byId('template-strategy').textContent = strategy.name;
      byId('template-params').textContent = `Params: ${JSON.stringify(strategy.params)}`;
      const configBody = byId('backtest-config-body'); configBody.replaceChildren();
      const config = strategy.backtest_config && typeof strategy.backtest_config === 'object'
        ? strategy.backtest_config : {};
      Object.entries(config).forEach(([key, value]) => {
        const row = document.createElement('tr');
        if (key === 'exchange' || key === 'initial_position') row.className = 'config-highlight';
        cell(row, key.replaceAll('_', ' ').replace(/\b\w/g, letter => letter.toUpperCase()));
        cell(row, typeof value === 'object' ? JSON.stringify(value) : String(value));
        configBody.appendChild(row);
      });
      if (!Object.keys(config).length) {
        const row = document.createElement('tr'); cell(row, 'No configuration recorded'); cell(row, '—');
        configBody.appendChild(row);
      }
      byId('strategy-source').textContent = strategy.source || 'strategy.rs unavailable';
    }

    function bindPager(key, loader) {
      byId(`${key}-size`).addEventListener('change', async event => {
        pageSize[key] = Number(event.target.value); page[key] = 1; await loader();
      });
      byId(`${key}-prev`).addEventListener('click', async () => {
        page[key] = Math.max(1, page[key] - 1); await loader();
      });
      byId(`${key}-next`).addEventListener('click', async () => {
        page[key] += 1; await loader();
      });
      const jump = async () => {
        const requested = Number(byId(`${key}-page`).value);
        if (Number.isFinite(requested)) page[key] = Math.max(1, Math.floor(requested));
        await loader();
      };
      byId(`${key}-page`).addEventListener('change', jump);
      byId(`${key}-page`).addEventListener('keydown', event => {
        if (event.key === 'Enter') { event.preventDefault(); jump(); }
      });
    }

    function bindSorting(key, loader) {
      const headers = byId(`${key}-table`).querySelectorAll('th[data-sort]');
      const renderIcon = () => headers.forEach(header => {
        header.classList.remove('sort-asc', 'sort-desc');
        if (header.dataset.sort === sorting[key].column) {
          header.classList.add(`sort-${sorting[key].order}`);
        }
      });
      headers.forEach(header => {
        header.addEventListener('click', async () => {
          const sameColumn = sorting[key].column === header.dataset.sort;
          sorting[key] = {
            column: header.dataset.sort,
            order: sameColumn && sorting[key].order === 'asc' ? 'desc' : 'asc'
          };
          page[key] = 1; renderIcon(); await loader();
        });
      });
      renderIcon();
    }

    bindPager('strategy', loadStrategies);
    bindPager('asset', loadAssets);
    bindPager('interval', loadIntervals);
    bindSorting('strategy', loadStrategies);
    bindSorting('asset', loadAssets);
    bindSorting('interval', loadIntervals);
    byId('asset-expand').addEventListener('click', () => {
      window.open(`/asset-comparison?strategy=${selectedStrategy}`, '_blank');
    });

    async function start() {
      try {
        const meta = await loadMeta();
        if (meta.strategies === 0) {
          byId('selected-strategy').textContent = 'Waiting for completed sweep batch…';
          setTimeout(init, 1000);
          return;
        }
        await loadStrategies();
        await loadSource();
        await loadAssets();
      } catch (error) {
        const item = document.createElement('div'); item.className = 'warning';
        item.textContent = `Load failed: ${error.message}`; byId('warnings').appendChild(item);
      }
    }
    start();
  </script>
</body>
</html>
"#;
