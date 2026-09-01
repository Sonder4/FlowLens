<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "./lib/tauri";
  import type { PolicyStatus } from "./lib/tauri";
  import { initState, selectDevice, state as appState } from "./lib/state.svelte";

  let devices: { name: string; display: string }[] = $state([]);
  let policy = $state<PolicyStatus | null>(null);
  let policyMsg = $state("");
  let policyErr = $state(false);
  let busy = $state(false);
  let confirmMode = $state<string | null>(null);
  let autostart = $state(false);
  let autoBusy = $state(false);
  let autoErr = $state("");

  onMount(async () => {
    // 等待后端 setup 完成（initState 幂等，主视图已调用过则直接返回）
    await initState();
    const list = await api.listDevices();
    devices = list.map((d) => ({ name: d.name, display: d.desc ?? d.name }));
    refreshPolicy();
    refreshAutostart();
  });

  async function refreshAutostart(): Promise<void> {
    try {
      autostart = await api.autostartStatus();
    } catch {
      autostart = false;
    }
  }

  async function toggleAutostart(): Promise<void> {
    autoBusy = true;
    autoErr = "";
    try {
      await api.setAutostart(!autostart);
      autostart = await api.autostartStatus();
    } catch (e) {
      autoErr = String(e);
    }
    autoBusy = false;
  }

  async function refreshPolicy(): Promise<void> {
    policy = await api.ipv6PolicyStatus();
  }

  // 触发 UAC 弹窗以管理员身份重启：确认成功后当前进程会退出
  async function restartAsAdmin(): Promise<void> {
    busy = true;
    policyErr = false;
    policyMsg = "";
    try {
      await api.restartAsAdmin();
      // 成功路径下后端已退出进程，通常不会走到这里
    } catch (e) {
      policyErr = true;
      policyMsg = String(e);
      busy = false;
    }
  }

  async function applyPolicy(mode: string): Promise<void> {
    // 未提权时：第一次点击提示，再次点击触发 UAC 提权重启
    if (policy && !policy.elevated) {
      if (confirmMode !== mode) {
        confirmMode = mode;
        return;
      }
      confirmMode = null;
      await restartAsAdmin();
      return;
    }
    // 仅 IPv6-only / IPv4-only 这类会断网的设置需要二次确认
    if ((mode === "ipv6_only" || mode === "ipv4_only") && confirmMode !== mode) {
      confirmMode = mode;
      return;
    }
    confirmMode = null;
    busy = true;
    policyErr = false;
    policyMsg = "";
    try {
      policyMsg = await api.setIpv6Policy(mode);
    } catch (e) {
      policyErr = true;
      policyMsg = String(e);
    }
    busy = false;
    refreshPolicy();
  }
</script>

<div class="settings glass">
  <div class="head">
    <span class="title">设置</span>
  </div>

  <section class="group">
    <div class="group-title">抓包</div>
    <div class="item">
      <span>捕获范围</span>
      <select
        value={appState.activeDevice ?? ""}
        onchange={(e) => selectDevice(e.currentTarget.value || null)}
      >
        <option value="">全部网卡</option>
        {#each devices as d (d.name)}
          <option value={d.display}>{d.display}</option>
        {/each}
      </select>
    </div>
    <p class="hint">切换后立即以新范围重启抓包；IPv4/IPv6 区分依赖抓包引擎（Npcap）。</p>
  </section>

  <section class="group">
    <div class="group-title">IP 协议策略</div>
    {#if policy}
      <div class="policy-status">
        <div class="line">
          前缀策略：::/0 优先级 {policy.v6Precedence} · IPv4(::ffff:0:0/96) 优先级 {policy.v4Precedence}
          {#if policy.preferIpv6}<span class="badge v6">IPv6 优先</span>{:else}<span class="badge v4">非 IPv6 优先</span>{/if}
          {#if !policy.elevated}
          <span class="badge warn">未提升权限</span>
          <button class="btn" disabled={busy} onclick={() => restartAsAdmin()}>以管理员身份重启（UAC）</button>
        {/if}
        </div>
        <div class="line">
          网卡绑定：
          {#each policy.adapters as a (a.name)}
            <span class="adp">{a.name} · v4 {a.ipv4 ? "启用" : "禁用"} · v6 {a.ipv6 ? "启用" : "禁用"}</span>
          {:else}
            <span class="adp">未读到活动物理网卡</span>
          {/each}
        </div>
        {#if policy.error}<div class="line err">{policy.error}</div>{/if}
      </div>
    {/if}
    <div class="policy-btns">
      <button class="btn" disabled={busy} onclick={() => applyPolicy("prefer_ipv6")}>
        {policy && !policy.elevated && confirmMode === "prefer_ipv6" ? "再次点击：UAC 提权重启后生效" : "IPv6 优先"}
      </button>
      <button class="btn" disabled={busy} onclick={() => applyPolicy("prefer_ipv4")}>
        {policy && !policy.elevated && confirmMode === "prefer_ipv4" ? "再次点击：UAC 提权重启后生效" : "IPv4 优先"}
      </button>
      <button class="btn danger" disabled={busy} onclick={() => applyPolicy("ipv6_only")}>
        {confirmMode === "ipv6_only"
          ? policy && !policy.elevated ? "再次点击：UAC 提权重启后生效" : "再次点击确认（会禁用 IPv4）"
          : "IPv6-only"}
      </button>
      <button class="btn danger" disabled={busy} onclick={() => applyPolicy("ipv4_only")}>
        {confirmMode === "ipv4_only"
          ? policy && !policy.elevated ? "再次点击：UAC 提权重启后生效" : "再次点击确认（会禁用 IPv6）"
          : "IPv4-only"}
      </button>
      <button class="btn" disabled={busy} onclick={() => applyPolicy("restore_dual")}>
        {policy && !policy.elevated && confirmMode === "restore_dual" ? "再次点击：UAC 提权重启后生效" : "恢复双栈"}
      </button>
    </div>
    {#if policyMsg}<p class="hint" class:err={policyErr}>{policyMsg}</p>{/if}
    <p class="hint">
      IPv6 优先 / IPv4 优先：netsh 前缀策略调整（重启后保留），仅决定双栈目标的连接优先序；
      IPv6-only / IPv4-only：禁用活动无线网卡的对应协议绑定，可能中断当前连接，需重新连接 Wi-Fi，
      IPv4-only 网站在 IPv6-only 下将无法访问。
      {#if policy && !policy.elevated}
        当前未以管理员运行：点击策略按钮会先弹出 UAC 提权确认，确认后 FlowLens
        将关闭并以管理员身份重新启动（取消 UAC 则保持现状）；也可以直接点击上方「以管理员身份重启」。
      {:else}
        修改需要管理员权限。
      {/if}
    </p>
  </section>

  <section class="group">
    <div class="group-title">窗口</div>
    <div class="item">
      <span>悬浮窗</span>
      <button class="btn" onclick={() => api.showWindow("floating")}>显示悬浮窗</button>
    </div>
    <div class="item">
      <span>主面板</span>
      <button class="btn" onclick={() => api.showWindow("main")}>显示主面板</button>
    </div>
  </section>

  <section class="group">
    <div class="group-title">启动</div>
    <div class="item">
      <span>开机自启</span>
      <button class="btn" class:primary={autostart} disabled={autoBusy} onclick={() => toggleAutostart()}>
        {autostart ? "已开启（点击关闭）" : "已关闭（点击开启）"}
      </button>
    </div>
    {#if autoErr}<p class="hint err">{autoErr}</p>{/if}
    <p class="hint">
      开启后登录 Windows 时自动运行 FlowLens，并以静默方式启动到系统托盘（不弹窗口），
      可随时从托盘图标打开主面板；对应注册表 HKCU\…\Run 下的 FlowLens 项。
    </p>
  </section>

  <section class="group">
    <div class="group-title">数据</div>
    <p class="hint">
      流量历史按分钟落盘（保留 90 天），按天/月汇总永久保留；
      默认数据库位于 %APPDATA%\flowlens\traffic_history.db，
      可通过环境变量 FLOWLENS_DATA_DIR 将数据目录指到其他磁盘。
    </p>
  </section>

  <footer class="foot">FlowLens v0.1 · 开源 · MIT</footer>
</div>

<style>
  .settings {
    position: relative;
    z-index: 1;
    /* 作为主窗口内嵌视图：由 App.svelte 的主区域负责宽度与滚动 */
    padding: 16px 18px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .head {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  .title { font-size: 17px; font-weight: 700; }
  .close {
    border: none;
    background: rgba(0, 0, 0, 0.05);
    border-radius: 8px;
    width: 26px;
    height: 26px;
    cursor: pointer;
    color: var(--text-secondary);
  }
  .group {
    border-top: 1px solid rgba(0, 0, 0, 0.06);
    padding-top: 10px;
  }
  .group-title {
    font-size: var(--fs-xs);
    color: var(--text-tertiary);
    text-transform: uppercase;
    letter-spacing: 0.8px;
    margin-bottom: 8px;
  }
  .item {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 4px 0;
    font-size: var(--fs-md);
  }
  select, .btn {
    font-family: inherit;
    font-size: var(--fs-sm);
    border: 1px solid rgba(0, 0, 0, 0.08);
    background: #fff;
    border-radius: var(--radius-sm);
    padding: 5px 10px;
    cursor: pointer;
    color: var(--text-primary);
  }
  .hint {
    font-size: var(--fs-xs);
    color: var(--text-tertiary);
    line-height: 1.6;
  }
  .hint.err { color: #d70015; }
  .policy-status {
    font-size: var(--fs-sm);
    line-height: 1.8;
    margin-bottom: 8px;
  }
  .policy-status .line { display: flex; align-items: center; flex-wrap: wrap; gap: 6px; }
  .policy-status .line.err { color: #d70015; }
  .adp {
    display: inline-block;
    background: rgba(0, 0, 0, 0.05);
    border-radius: 6px;
    padding: 1px 8px;
    font-size: var(--fs-xs);
  }
  .badge.warn { background: var(--orange); }
  .policy-btns {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin: 8px 0;
  }
  .btn.danger {
    color: #d70015;
    border-color: rgba(215, 0, 21, 0.35);
  }
  .btn:disabled { opacity: 0.5; cursor: default; }
  .btn.primary {
    background: var(--accent-v4, #2f7cf6);
    border-color: var(--accent-v4, #2f7cf6);
    color: #fff;
  }
  .foot {
    margin-top: auto;
    font-size: var(--fs-xs);
    color: var(--text-tertiary);
    text-align: center;
  }
</style>
