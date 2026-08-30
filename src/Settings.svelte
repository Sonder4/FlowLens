<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "./lib/tauri";
  import { selectDevice, state as appState } from "./lib/state.svelte";

  let devices: { name: string; display: string }[] = $state([]);

  onMount(async () => {
    await state;
    const list = await api.listDevices();
    devices = list.map((d) => ({ name: d.name, display: d.desc ?? d.name }));
  });
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
    <div class="group-title">数据</div>
    <p class="hint">
      流量历史按分钟落盘（保留 90 天），按天/月汇总永久保留；
      数据库位于 %APPDATA%\glassnet\traffic_history.db。
    </p>
  </section>

  <footer class="foot">GlassNet v0.1 · 基于 Sniffnet 二次开发 · MIT/Apache-2.0</footer>
</div>

<style>
  .settings {
    position: relative;
    z-index: 1;
    width: calc(100vw - 24px);
    height: calc(100vh - 24px);
    margin: 12px;
    padding: 16px 18px;
    display: flex;
    flex-direction: column;
    gap: 14px;
    overflow: auto;
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
  .foot {
    margin-top: auto;
    font-size: var(--fs-xs);
    color: var(--text-tertiary);
    text-align: center;
  }
</style>
