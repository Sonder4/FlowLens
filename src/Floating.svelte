<script lang="ts">
  import { onMount } from "svelte";
  import { api, fmtSpeed, listen } from "./lib/tauri";
  import type { AdapterIo } from "./lib/tauri";

  let io: AdapterIo[] = $state([]);
  let visible = $state(true);

  const total = $derived(
    io.reduce((acc, a) => ({ rx: acc.rx + a.rxSpeed, tx: acc.tx + a.txSpeed }), { rx: 0, tx: 0 }),
  );
  const top = $derived(io.filter((a) => a.rxSpeed > 0 || a.txSpeed > 0).slice(0, 3));

  onMount(() => {
    io = api.ioSnapshot ? io : io;
    api.ioSnapshot().then((s) => (io = s));
    listen<AdapterIo[]>("io-tick", (s) => (io = s));
  });

  function hide(): void {
    visible = false;
    api.hideWindow("floating");
    // 再显示由主面板侧边栏触发
    setTimeout(() => (visible = true), 500);
  }

  function drag(event: MouseEvent): void {
    if ((event.target as HTMLElement).closest("button")) return;
    import("@tauri-apps/api/window").then(({ getCurrentWindow }) => {
      getCurrentWindow().startDragging();
    });
  }
</script>

<svelte:window onmousedown={null} />

<div
  class="floating glass"
  role="application"
  onmousedown={drag}
>
  <div class="row main-row">
    <span class="dir rx">▼ {fmtSpeed(total.rx)}</span>
    <span class="dir tx">▲ {fmtSpeed(total.tx)}</span>
    <button class="mini" title="打开主面板" onclick={() => api.showWindow("main")}>主面板</button>
    <button class="mini" title="隐藏悬浮窗" onclick={hide}>✕</button>
  </div>
  {#each top as a (a.name)}
    <div class="row sub-row">
      <span class="name">{a.name}</span>
      <span class="num pair">▼ {fmtSpeed(a.rxSpeed)} · ▲ {fmtSpeed(a.txSpeed)}</span>
    </div>
  {:else}
    <div class="row sub-row"><span class="name">等待网络数据…</span></div>
  {/each}
</div>

<style>
  .floating {
    width: 100vw;
    height: 100vh;
    padding: 10px 14px;
    border-radius: var(--radius-md);
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 5px;
    cursor: move;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .main-row { justify-content: space-between; }
  .dir {
    font-size: 16px;
    font-weight: 700;
    font-variant-numeric: tabular-nums;
  }
  .rx { color: var(--accent-v4); }
  .tx { color: var(--orange); }
  .mini {
    border: none;
    background: rgba(0, 0, 0, 0.05);
    color: var(--text-secondary);
    border-radius: 8px;
    padding: 3px 9px;
    font-size: var(--fs-xs);
    font-family: inherit;
    cursor: pointer;
    transition: all 0.25s ease;
  }
  .mini:hover { background: rgba(0, 0, 0, 0.1); color: var(--text-primary); }
  .sub-row { justify-content: space-between; }
  .name {
    font-size: var(--fs-xs);
    color: var(--text-secondary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pair {
    font-size: var(--fs-xs);
    color: var(--text-tertiary);
  }
</style>
