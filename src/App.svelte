<script lang="ts">
  import { onMount } from "svelte";
  import {
    activeConnCount,
    displaySamples,
    displaySpeed,
    filteredFlows,
    initState,
    refreshHourly,
    selectDevice,
    sessionTotals,
    state as appState,
    v6Share,
  } from "./lib/state.svelte";
  import { api, fmtBytes, fmtSpeed, listen } from "./lib/tauri";
  import type { AppRangeRow, RangeSeries } from "./lib/tauri";
  import HourlyBars from "./lib/components/HourlyBars.svelte";
  import LiveCurve from "./lib/components/LiveCurve.svelte";
  import ConnTable from "./lib/components/ConnTable.svelte";
  import AppTrafficTable from "./lib/components/AppTrafficTable.svelte";
  import AppDailyHistory from "./lib/components/AppDailyHistory.svelte";
  import AppRangeTable from "./lib/components/AppRangeTable.svelte";
  import Settings from "./Settings.svelte";

  type View = "dash" | "inspect" | "history" | "settings";
  let collapsed = $state(false);
  let view: View = $state("dash");
  let devices: { name: string; display: string }[] = $state([]);

  const deviceDisplay: Record<string, string> = $state({});

  const segmentItems = $derived([
    { key: null, label: "全部" },
    ...devices.map((d) => ({ key: d.display, label: shorten(d.display) })),
  ]);

  function shorten(name: string): string {
    if (/wi-?fi|wlan|802\.11/i.test(name)) return "Wi-Fi";
    if (/以太网|ethernet|realtek|pcie/i.test(name)) return "以太网";
    if (/vpn|tap|tun/i.test(name)) return "VPN";
    return name.length > 10 ? `${name.slice(0, 9)}…` : name;
  }

  async function onSegment(key: string | null): Promise<void> {
    await selectDevice(key);
  }

  function setFilter(f: "v4" | "v6" | null): void {
    appState.familyFilter = appState.familyFilter === f ? null : f;
  }

  async function toggleCapture(): Promise<void> {
    if (appState.running) {
      await api.stopCapture();
      appState.running = false;
    } else {
      // 引擎始终捕获全部有地址网卡；网卡分段仅做前端显示过滤（切换零延迟）
      await api.startCapture(null);
      appState.running = true;
    }
  }

  // ---------- 历史页：任意时间范围查询 ----------
  type RangeKey = "today" | "yesterday" | "h24" | "d7" | "d30" | "month" | "custom";
  const RANGE_PRESETS: { key: RangeKey; label: string }[] = [
    { key: "today", label: "今天" },
    { key: "yesterday", label: "昨天" },
    { key: "h24", label: "近 24 小时" },
    { key: "d7", label: "近 7 天" },
    { key: "d30", label: "近 30 天" },
    { key: "month", label: "本月" },
    { key: "custom", label: "自定义" },
  ];
  const DAY_MS = 86_400_000;

  let rangeKey: RangeKey = $state("today");
  let rangeSeries: RangeSeries | null = $state(null);
  let appRows: AppRangeRow[] = $state([]);
  let rangeLoading = $state(false);
  let customFrom = $state("");
  let customTo = $state("");

  function rangeBounds(key: RangeKey): { since: number; until: number; title: string } {
    const now = new Date();
    const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate());
    const monthStart = new Date(now.getFullYear(), now.getMonth(), 1);
    const hm = (d: Date): string =>
      `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
    switch (key) {
      case "today":
        return { since: +startOfToday, until: +now, title: "今天" };
      case "yesterday": {
        const from = new Date(+startOfToday - DAY_MS);
        return { since: +from, until: +startOfToday, title: "昨天" };
      }
      case "h24":
        return { since: +now - DAY_MS, until: +now, title: "近 24 小时" };
      case "d7":
        return { since: +now - 7 * DAY_MS, until: +now, title: "近 7 天" };
      case "d30":
        return { since: +now - 30 * DAY_MS, until: +now, title: "近 30 天" };
      case "month":
        return { since: +monthStart, until: +now, title: `本月（${now.getMonth() + 1} 月）` };
      case "custom": {
        const from = customFrom ? new Date(customFrom) : new Date(+now - DAY_MS);
        const to = customTo ? new Date(customTo) : now;
        const pad = (d: Date): number => Math.floor(+d / 1000) * 1000;
        return {
          since: pad(from),
          until: pad(to),
          title: `自定义（${from.getMonth() + 1}/${from.getDate()} ${hm(from)} ~ ${to.getMonth() + 1}/${to.getDate()} ${hm(to)}）`,
        };
      }
    }
  }

  async function loadRange(): Promise<void> {
    const b = rangeBounds(rangeKey);
    rangeLoading = true;
    try {
      const [series, rows] = await Promise.all([
        api.historyRange(Math.floor(b.since / 1000), Math.ceil(b.until / 1000), null),
        api.historyAppRange(Math.floor(b.since / 1000), Math.ceil(b.until / 1000)),
      ]);
      rangeSeries = series;
      appRows = rows;
    } finally {
      rangeLoading = false;
    }
  }

  function applyPreset(key: RangeKey): void {
    rangeKey = key;
    // 自定义需要先填起止时间，填好后 applyCustom 触发
    if (key !== "custom" || (customFrom && customTo)) void loadRange();
  }

  function applyCustom(): void {
    if (!customFrom || !customTo) return;
    rangeKey = "custom";
    void loadRange();
  }

  // 范围内四类流量合计（来自应用归因明细）+ 图表总/峰值（网卡全量）
  const rangeSummary = $derived.by(() => {
    const sums = { system: 0, software: 0, dev: 0, other: 0 };
    for (const r of appRows) {
      sums[r.category] = (sums[r.category] ?? 0) + r.rxV4 + r.txV4 + r.rxV6 + r.txV6;
    }
    let total = 0;
    let peak = 0;
    for (const b of rangeSeries?.buckets ?? []) {
      const t = b.rxV4 + b.rxV6 + b.txV4 + b.txV6;
      total += t;
      peak = Math.max(peak, t);
    }
    return { sums, total, peak };
  });

  onMount(() => {
    initState();
    api.listDevices().then((list) => {
      devices = list
        .filter((d) => d.addresses.length > 0)
        .map((d) => ({ name: d.name, display: d.desc ?? d.name }))
        .filter((d) => !/bluetooth|wan miniport|loopback/i.test(d.display));
    });
    // 定期刷新 24 小时视图（每 5 分钟对齐一次后端持久化数据）
    const timer = setInterval(() => {
      refreshHourly();
    }, 5 * 60 * 1000);
    listen<{ running: boolean }>("capture-state", (p) => {
      appState.running = p.running;
    });
    return () => clearInterval(timer);
  });
</script>

<div class="app" class:collapsed>
  <!-- 侧边栏 -->
  <aside class="sidebar glass" class:collapsed>
    <div class="logo">
      <div class="logo-icon">
        <svg viewBox="0 0 512 512" width="19" height="19" fill="none" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
          <circle cx="256" cy="256" r="140" stroke="#fff" stroke-width="56"/>
          <g clip-path="url(#lensLogo)">
            <path d="M 122 256 C 170 202, 226 202, 256 256 C 286 310, 342 310, 390 256" stroke="#fff" stroke-width="44" stroke-linecap="round"/>
          </g>
          <defs><clipPath id="lensLogo"><circle cx="256" cy="256" r="119"/></clipPath></defs>
        </svg>
      </div>
      {#if !collapsed}
        <div>
          <div class="logo-name">FlowLens</div>
          <div class="logo-sub">流量监控 · v0.1</div>
        </div>
      {/if}
      <button class="collapse-btn" title={collapsed ? "展开侧边栏" : "收起侧边栏"}
              onclick={() => (collapsed = !collapsed)}>
        {collapsed ? "›" : "‹"}
      </button>
    </div>
    <nav class="nav">
      <button class="nav-item" class:active={view === "dash"} onclick={() => (view = "dash")}>
        <span class="icon">▤</span>{#if !collapsed}<span class="label">仪表盘</span>{/if}
      </button>
      <button class="nav-item" class:active={view === "inspect"} onclick={() => (view = "inspect")}>
        <span class="icon">⌕</span>{#if !collapsed}<span class="label">连接详情</span>{/if}
      </button>
      <button class="nav-item" class:active={view === "history"} onclick={() => { view = "history"; void loadRange(); }}>
        <span class="icon">◔</span>{#if !collapsed}<span class="label">历史记录</span>{/if}
      </button>
      <button class="nav-item" class:active={view === "settings"} onclick={() => (view = "settings")}>
        <span class="icon">⚙</span>{#if !collapsed}<span class="label">设置</span>{/if}
      </button>
    </nav>
    <div class="sidebar-footer">
      <span class="status-dot" class:off={!appState.running} />
      {#if !collapsed}{appState.running ? "抓包运行中" : "抓包已停止"}{/if}
      {#if appState.errorMsg}
        <div class="error num">{appState.errorMsg}</div>
      {/if}
    </div>
  </aside>

  <div class="main">
    <!-- 顶部工具栏 -->
    <header class="topbar glass">
      <div class="iface-switch">
        {#each segmentItems as seg (seg.key ?? "all")}
          <button
            class="iface-btn"
            class:active={appState.activeDevice === seg.key}
            onclick={() => onSegment(seg.key)}
          >{seg.label}</button>
        {/each}
      </div>
      <div class="spacer" />
      <div class="seg">
        <button
          class="iface-btn v4c"
          class:active={appState.familyFilter === "v4"}
          onclick={() => setFilter("v4")}
        >IPv4</button>
        <button
          class="iface-btn v6c"
          class:active={appState.familyFilter === "v6"}
          onclick={() => setFilter("v6")}
        >IPv6</button>
      </div>
      <button class="round-btn" title="悬浮窗" onclick={() => api.showWindow("floating")}>▫</button>
      <button class="round-btn" title={appState.running ? "停止抓包" : "开始抓包"} onclick={toggleCapture}>
        {appState.running ? "■" : "▶"}
      </button>
    </header>

    {#if view === "dash"}
      <!-- 统计卡 -->
      <section class="cards">
        <div class="card glass glass-hover">
          <div class="label">总下载</div>
          <div class="value num">{fmtBytes(sessionTotals().rx)}</div>
          <div class="delta num">▼ {fmtSpeed(displaySpeed().rx)}</div>
        </div>
        <div class="card glass glass-hover">
          <div class="label">总上传</div>
          <div class="value num">{fmtBytes(sessionTotals().tx)}</div>
          <div class="delta num up">▲ {fmtSpeed(displaySpeed().tx)}</div>
        </div>
        <div class="card glass glass-hover">
          <div class="label">
            IPv6 占比
            {#if v6Share().v4 > 0 && v6Share().v6 > 0}<span class="badge dual">DUAL</span>{/if}
          </div>
          <div class="value num">{v6Share().pct}%</div>
          <div class="delta num">IPv4 {fmtBytes(v6Share().v4)} · IPv6 {fmtBytes(v6Share().v6)}</div>
        </div>
        <div class="card glass glass-hover">
          <div class="label">活跃连接</div>
          <div class="value num">{activeConnCount().total}</div>
          <div class="delta num">IPv4 {activeConnCount().v4} · IPv6 {activeConnCount().v6}</div>
        </div>
      </section>

      <!-- 图表行 -->
      <section class="charts">
        <div class="panel glass">
          <div class="panel-head">
            <span class="panel-title">24 小时流量</span>
            <span class="panel-sub num">总计 {fmtBytes(v6Share().total)} · {appState.hourly.length} 桶 · 峰值 {fmtBytes(Math.max(0, ...appState.hourly.map((b) => (b.rxV4 || 0) + (b.rxV6 || 0) + (b.txV4 || 0) + (b.txV6 || 0))))}</span>
          </div>
          <HourlyBars data={appState.hourly} />
        </div>
        <div class="panel glass">
          <LiveCurve samples={displaySamples()} />
        </div>
      </section>

      <!-- 应用流量（按进程 IPv4/IPv6 明细） -->
      <section class="apps">
        <div class="panel glass">
          <div class="panel-head">
            <span class="panel-title">应用流量（IPv4 / IPv6）</span>
            <span class="panel-sub">按进程累计 · 「其他」可展开查看归属未识别的连接</span>
          </div>
          <AppTrafficTable flows={filteredFlows()} rows={6} />
        </div>
      </section>

      <!-- 连接面板 -->
      <section class="conns">
        <ConnTable compact flows={filteredFlows()} rows={6} />
      </section>
    {:else if view === "inspect"}
      <!-- 连接详情页：应用流量明细 + 逐条流 -->
      <section class="stack">
        <div class="panel glass">
          <div class="panel-head">
            <span class="panel-title">应用流量明细（IPv4 / IPv6）</span>
            <span class="panel-sub">按进程累计 · 尊重当前网卡与 IPv4/IPv6 过滤</span>
          </div>
          <AppTrafficTable flows={filteredFlows()} rows={10} />
        </div>
        <ConnTable flows={filteredFlows()} rows={12} />
      </section>
    {:else if view === "settings"}
      <!-- 设置页：内嵌主窗口，与其它侧边视图一致，不再弹出独立窗口 -->
      <section class="settings-view">
        <Settings />
      </section>
    {:else}
      <!-- 历史记录页：任意时间范围 + 分类汇总 + 应用明细 -->
      <section class="history-panel glass">
        <div class="range-head">
          <div class="range-presets">
            {#each RANGE_PRESETS as p (p.key)}
              <button class="range-btn" class:active={rangeKey === p.key}
                      onclick={() => applyPreset(p.key)}>{p.label}</button>
            {/each}
          </div>
          {#if rangeKey === "custom"}
            <div class="custom-range">
              <input type="datetime-local" bind:value={customFrom} onchange={() => applyCustom()} />
              <span>至</span>
              <input type="datetime-local" bind:value={customTo} onchange={() => applyCustom()} />
            </div>
          {/if}
        </div>
        <section class="cat-cards">
          <div class="cat-card glass">
            <div class="label"><span class="badge-dot system" />系统流量</div>
            <div class="value num">{fmtBytes(rangeSummary.sums.system)}</div>
          </div>
          <div class="cat-card glass">
            <div class="label"><span class="badge-dot software" />软件流量</div>
            <div class="value num">{fmtBytes(rangeSummary.sums.software)}</div>
          </div>
          <div class="cat-card glass">
            <div class="label"><span class="badge-dot dev" />开发流量</div>
            <div class="value num">{fmtBytes(rangeSummary.sums.dev)}</div>
          </div>
          <div class="cat-card glass">
            <div class="label"><span class="badge-dot other" />未归类</div>
            <div class="value num">{fmtBytes(rangeSummary.sums.other)}</div>
          </div>
        </section>
        <div class="panel-head">
          <span class="panel-title">{rangeBounds(rangeKey).title}流量</span>
          <span class="panel-sub num">
            {#if rangeLoading}加载中…{:else}总计 {fmtBytes(rangeSummary.total)} · {rangeSeries?.buckets.length ?? 0} 桶 · 峰值 {fmtBytes(rangeSummary.peak)}{/if}
          </span>
        </div>
        <HourlyBars data={rangeSeries?.buckets ?? []} granularity={rangeSeries?.granularity ?? "hour"} />
      </section>
      <!-- 范围内应用流量明细（全量无门槛，四类筛选） -->
      <section class="history-panel glass">
        <div class="panel-head">
          <span class="panel-title">应用流量明细（{rangeBounds(rangeKey).title}）</span>
          <span class="panel-sub">按进程累计 · IPv4 / IPv6 分列 · 四类筛选</span>
        </div>
        <AppRangeTable rows={appRows} loading={rangeLoading} />
      </section>
      <!-- 应用每日流量历史（SQLite 持久化，重启后可查） -->
      <section class="history-panel glass">
        <div class="panel-head">
          <span class="panel-title">应用每日流量（当日合计 &gt; 100 MB）</span>
          <span class="panel-sub">本地数据库持久化 · IPv4 / IPv6 分列</span>
        </div>
        <AppDailyHistory />
      </section>
    {/if}
  </div>
</div>

<style>
  .app {
    position: relative;
    z-index: 1;
    display: grid;
    /* 侧边栏列宽随收起状态变化（元素宽度交给网格轨道决定），主区域始终贴随其后；
       grid-template-columns 可插值，收起/展开有平滑过渡 */
    grid-template-columns: 220px 1fr;
    grid-template-rows: 52px 1fr;
    gap: 16px;
    padding: 16px;
    height: 100vh;
    transition: grid-template-columns 0.25s ease;
  }
  .app.collapsed { grid-template-columns: 58px 1fr; }
  .main {
    grid-row: 1 / 3;
    /* 内容整体滚动（超出视口可上下滑动），侧边栏与顶栏保持固定 */
    display: flex;
    flex-direction: column;
    gap: 16px;
    overflow-y: auto;
    min-height: 0;
  }
  /* flex 子项默认会收缩挤压面板造成内容重叠，这里禁止收缩、按内容自然撑高 */
  .main > * { flex-shrink: 0; }

  /* 侧边栏：宽度由 .app 的网格轨道驱动，这里不再单独设 width */
  .sidebar {
    grid-row: 1 / 3;
    display: flex;
    flex-direction: column;
    padding: 18px 12px 14px;
    min-width: 0;
  }
  .sidebar.collapsed { padding: 18px 8px 14px; }
  .collapse-btn {
    margin-left: auto;
    border: none;
    background: transparent;
    color: var(--text-tertiary);
    font-size: 14px;
    cursor: pointer;
    border-radius: 6px;
    width: 22px;
    height: 22px;
    transition: all 0.25s ease;
  }
  .collapse-btn:hover { background: rgba(0, 0, 0, 0.06); color: var(--text-primary); }
  /* 收起态：logo 图标与展开按钮纵向排列，避免 58px 栏宽内横向溢出 */
  .collapsed .logo {
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 4px 0 14px;
  }
  .collapsed .collapse-btn { margin-left: 0; }
  .collapsed .nav-item { justify-content: center; padding: 9px 6px; }
  .collapsed .nav-item .label { display: none; }
  .collapsed .nav-item .badge { margin-left: 0; }
  .nav-item .badge { margin-left: auto; }
  .logo {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 4px 10px 18px;
  }
  .logo-icon {
    width: 32px;
    height: 32px;
    border-radius: 9px;
    background: linear-gradient(135deg, var(--accent-v4), var(--accent-v6));
    color: #fff;
    font-weight: 700;
    font-size: 16px;
    display: flex;
    align-items: center;
    justify-content: center;
    box-shadow: 0 2px 8px rgba(0, 113, 227, 0.3);
  }
  .logo-name { font-size: var(--fs-lg); font-weight: 700; letter-spacing: 0.2px; }
  .logo-sub { font-size: var(--fs-xs); color: var(--text-tertiary); }
  .nav { display: flex; flex-direction: column; gap: 4px; margin-top: 6px; }
  .nav-item {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 9px 12px;
    border-radius: var(--radius-sm);
    border: none;
    background: transparent;
    color: var(--text-secondary);
    font-weight: 500;
    font-size: var(--fs-md);
    font-family: inherit;
    cursor: pointer;
    transition: all 0.25s ease;
    text-align: left;
  }
  .nav-item:hover { background: rgba(255, 255, 255, 0.7); color: var(--text-primary); }
  .nav-item.active {
    background: #fff;
    color: var(--text-primary);
    font-weight: 600;
    box-shadow: 0 2px 8px rgba(30, 40, 60, 0.08);
  }
  .icon { width: 18px; text-align: center; font-size: 14px; }
  .sidebar-footer {
    margin-top: auto;
    padding: 12px 10px 4px;
    border-top: 1px solid rgba(0, 0, 0, 0.06);
    font-size: var(--fs-xs);
    color: var(--text-tertiary);
  }
  .status-dot {
    display: inline-block;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--green);
    margin-right: 6px;
    box-shadow: 0 0 6px rgba(52, 199, 89, 0.6);
  }
  .status-dot.off { background: var(--text-tertiary); box-shadow: none; }
  .error { color: var(--orange); margin-top: 6px; font-size: var(--fs-xs); }

  /* 顶栏 */
  .topbar {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 0 12px;
    border-radius: var(--radius-md);
  }
  .spacer { flex: 1; }
  .iface-switch, .seg {
    display: flex;
    gap: 4px;
    background: rgba(0, 0, 0, 0.05);
    padding: 3px;
    border-radius: var(--radius-sm);
  }
  .iface-btn {
    padding: 5px 14px;
    border-radius: 8px;
    border: none;
    background: transparent;
    color: var(--text-secondary);
    font-size: var(--fs-sm);
    font-weight: 600;
    cursor: pointer;
    font-family: inherit;
    transition: all 0.25s ease;
  }
  .iface-btn.active { background: #fff; color: var(--text-primary); box-shadow: 0 1px 4px rgba(30, 40, 60, 0.12); }
  .iface-btn.v4c.active { color: var(--accent-v4); }
  .iface-btn.v6c.active { color: var(--accent-v6); }
  .round-btn {
    width: 32px;
    height: 32px;
    border-radius: 50%;
    border: none;
    background: #fff;
    color: var(--text-primary);
    font-size: 12px;
    cursor: pointer;
    box-shadow: 0 1px 4px rgba(30, 40, 60, 0.12);
    transition: all 0.25s ease;
  }
  .round-btn:hover { transform: translateY(-1px); }

  /* 统计卡 */
  .cards {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 16px;
  }
  .card {
    padding: 14px 18px;
  }
  .card:hover { transform: translateY(-2px); }
  .label {
    font-size: var(--fs-xs);
    color: var(--text-tertiary);
    text-transform: uppercase;
    letter-spacing: 0.8px;
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .badge.dual { background: linear-gradient(135deg, var(--accent-v4), var(--accent-v6)); }
  .value {
    font-size: var(--fs-xl);
    font-weight: 700;
    margin: 4px 0 2px;
  }
  .delta {
    font-size: var(--fs-xs);
    color: var(--text-secondary);
  }
  .delta.up, .delta .up { color: var(--orange); }

  /* 图表行：固定高度，图表在面板内按比例缩放，不撑破布局 */
  .charts {
    display: grid;
    grid-template-columns: 1.8fr 1fr;
    gap: 16px;
    height: 240px;
    min-height: 0;
  }
  .panel {
    padding: 14px 18px 10px;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .panel-head {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    margin-bottom: 6px;
  }
  .panel-title { font-size: var(--fs-lg); font-weight: 600; }
  .panel-sub { font-size: var(--fs-xs); color: var(--text-tertiary); }

  .conns { min-height: 0; }

  .apps { min-height: 0; }

  /* 连接详情页：上下两块面板，随整页滚动完整展示（子面板禁止收缩避免内容重叠） */
  .stack {
    display: flex;
    flex-direction: column;
    gap: 16px;
    min-height: 0;
  }
  .stack > * { flex-shrink: 0; }

  .settings-view {
    display: flex;
    flex-direction: column;
  }

  .history-panel {
    flex: 1 0 auto;
    padding: 18px;
    display: flex;
    flex-direction: column;
  }

  /* 历史页：时间范围选择 + 分类汇总 */
  .range-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    flex-wrap: wrap;
    margin-bottom: 12px;
  }
  .range-presets { display: flex; gap: 4px; flex-wrap: wrap; }
  .range-btn {
    padding: 5px 14px;
    border: none;
    border-radius: 8px;
    background: rgba(0, 0, 0, 0.05);
    color: var(--text-secondary);
    font-size: var(--fs-sm);
    font-weight: 600;
    font-family: inherit;
    cursor: pointer;
    transition: all 0.25s ease;
  }
  .range-btn:hover { background: rgba(255, 255, 255, 0.9); color: var(--text-primary); }
  .range-btn.active {
    background: #fff;
    color: var(--text-primary);
    box-shadow: 0 1px 4px rgba(30, 40, 60, 0.12);
  }
  .custom-range {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: var(--fs-sm);
    color: var(--text-secondary);
  }
  .custom-range input {
    border: 1px solid rgba(0, 0, 0, 0.1);
    border-radius: 8px;
    padding: 4px 8px;
    font: inherit;
    font-size: var(--fs-sm);
    background: rgba(255, 255, 255, 0.85);
    color: var(--text-primary);
  }
  .cat-cards {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 12px;
    margin-bottom: 14px;
  }
  .cat-card { padding: 10px 14px; }
  .cat-card .value { font-size: var(--fs-lg); margin: 2px 0 0; }
  .badge-dot {
    display: inline-block;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    margin-right: 2px;
  }
  .badge-dot.system { background: #6b7a90; }
  .badge-dot.software { background: var(--accent-v4, #2f7cf6); }
  .badge-dot.dev { background: #8e6bd6; }
  .badge-dot.other { background: #b0b4bc; }
</style>
