<script lang="ts">
  import { onMount } from "svelte";
  import {
    activeConnCount,
    appTop,
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
  import HourlyBars from "./lib/components/HourlyBars.svelte";
  import LiveCurve from "./lib/components/LiveCurve.svelte";
  import ConnTable from "./lib/components/ConnTable.svelte";

  type View = "dash" | "inspect" | "history";
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

<div class="app">
  <!-- 侧边栏 -->
  <aside class="sidebar glass" class:collapsed>
    <div class="logo">
      <div class="logo-icon">G</div>
      {#if !collapsed}
        <div>
          <div class="logo-name">GlassNet</div>
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
      <button class="nav-item" class:active={view === "history"} onclick={() => { view = "history"; refreshHourly(); }}>
        <span class="icon">◔</span>{#if !collapsed}<span class="label">历史记录</span>{/if}
      </button>
      <button class="nav-item" onclick={() => api.showWindow("settings")}>
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

      <!-- 应用流量 TOP -->
      <section class="apps">
        <div class="panel glass">
          <div class="panel-head">
            <span class="panel-title">应用流量 TOP</span>
            <span class="panel-sub">按累计 · 「连接详情」页可查看逐条流</span>
          </div>
          <div class="apps-row">
            {#each appTop().slice(0, 5) as a (a.program)}
              <div class="app-card">
                <div class="app-name" title={a.program}>{a.program}</div>
                <div class="app-badges">
                  {#if a.v4}<span class="badge v4">IPv4</span>{/if}
                  {#if a.v6}<span class="badge v6">IPv6</span>{/if}
                </div>
                <div class="app-num num">▼ {fmtBytes(a.rx)}</div>
                <div class="app-num num up">▲ {fmtBytes(a.tx)}</div>
              </div>
            {:else}
              <div class="app-empty">暂无应用数据 — 抓包运行后按进程聚合显示</div>
            {/each}
          </div>
        </div>
      </section>

      <!-- 连接面板 -->
      <section class="conns">
        <ConnTable compact flows={filteredFlows()} rows={6} />
      </section>
    {:else if view === "inspect"}
      <!-- 连接详情页 -->
      <section class="history-panel glass">
        <ConnTable flows={filteredFlows()} rows={20} />
      </section>
    {:else}
      <!-- 历史记录页 -->
      <section class="history-panel glass">
        <div class="panel-head">
          <span class="panel-title">24 小时流量历史</span>
          <span class="panel-sub num">总计 {fmtBytes(v6Share().total)}</span>
        </div>
        <HourlyBars data={appState.hourly} />
      </section>
    {/if}
  </div>
</div>

<style>
  .app {
    position: relative;
    z-index: 1;
    display: grid;
    grid-template-columns: 220px 1fr;
    grid-template-rows: 52px 1fr;
    gap: 16px;
    padding: 16px;
    height: 100vh;
  }
  .main {
    grid-row: 1 / 3;
    display: grid;
    grid-template-rows: 52px auto minmax(0, 1.25fr) auto minmax(0, 1fr);
    gap: 16px;
    height: 100%;
    min-height: 0;
    overflow: hidden;
  }

  /* 侧边栏 */
  .sidebar {
    grid-row: 1 / 3;
    display: flex;
    flex-direction: column;
    padding: 18px 12px 14px;
    width: 220px;
    transition: width 0.25s ease;
  }
  .sidebar.collapsed { width: 58px; padding: 18px 8px 14px; }
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
  .collapsed .logo { padding-bottom: 14px; }
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

  /* 图表 */
  .charts {
    display: grid;
    grid-template-columns: 1.8fr 1fr;
    gap: 16px;
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
  .apps-row { display: flex; gap: 12px; overflow: hidden; }
  .app-card {
    flex: 1;
    min-width: 0;
    background: rgba(255, 255, 255, 0.5);
    border: 1px solid var(--glass-edge);
    border-radius: var(--radius-md);
    padding: 10px 12px;
  }
  .app-name {
    font-weight: 600;
    font-size: var(--fs-md);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .app-badges { display: flex; gap: 4px; margin: 5px 0; }
  .app-num { font-size: var(--fs-sm); color: var(--accent-v4); }
  .app-num.up { color: var(--orange); }
  .app-empty {
    flex: 1;
    text-align: center;
    color: var(--text-tertiary);
    padding: 14px 0;
    font-size: var(--fs-sm);
  }

  .history-panel {
    padding: 18px;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
</style>
