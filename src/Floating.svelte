<script lang="ts">
  import { onMount } from "svelte";
  import { api, fmtSpeed, listen } from "./lib/tauri";
  import type { AdapterIo } from "./lib/tauri";

  let io: AdapterIo[] = $state([]);
  let peak = $state(1024 * 1024); // 速率条归一化上限：随实际峰值自适应
  let addrs: string[] = $state([]);

  const total = $derived(
    io.reduce((acc, a) => ({ rx: acc.rx + a.rxSpeed, tx: acc.tx + a.txSpeed }), { rx: 0, tx: 0 }),
  );

  // 速率条宽度：相对近 60 秒峰值
  let hist: number[] = [];
  const dlPct = $derived.by(() => {
    hist.push(total.rx);
    if (hist.length > 60) hist.shift();
    peak = Math.max(1024 * 1024, ...hist);
    return Math.min(100, (total.rx / peak) * 100);
  });
  const ulPct = $derived(Math.min(100, (total.tx / peak) * 100));

  // v4/v6 栈状态：按本机实际地址判断亮灭
  const v4addr = $derived(addrs.find((a) => !a.includes(":")));
  const v6addr = $derived(addrs.find((a) => a.includes(":")));
  const v4on = $derived(!!v4addr);
  const v6on = $derived(!!v6addr);

  onMount(() => {
    api.ioSnapshot().then((s) => (io = s));
    listen<AdapterIo[]>("io-tick", (s) => (io = s));
    api.localAddresses().then((a) => (addrs = a));
  });

  function drag(event: MouseEvent): void {
    if ((event.target as HTMLElement).closest("button")) return;
    import("@tauri-apps/api/window").then(({ getCurrentWindow }) => {
      getCurrentWindow().startDragging();
    });
  }

  function popup(event: MouseEvent): void {
    event.preventDefault();
    api.popupFloatingMenu();
  }
</script>

<div
  class="widget glass"
  role="application"
  onmousedown={drag}
  oncontextmenu={popup}
>
  <div class="row">
    <span class="icon dl">↓</span>
    <span class="num numv">{fmtSpeed(total.rx)}</span>
    <span class="bar"><span class="fill fill-dl" style="width:{dlPct}%" /></span>
  </div>
  <div class="row">
    <span class="icon ul">↑</span>
    <span class="num numv">{fmtSpeed(total.tx)}</span>
    <span class="bar"><span class="fill fill-ul" style="width:{ulPct}%" /></span>
  </div>
  <div class="ip-row">
    <span class="ip-chip" class:chip-on={v4on}><span class="pulse" />v4</span>
    <span class="ip-chip" class:chip-on={v6on}><span class="pulse" />v6</span>
    <span class="dual">{v4on && v6on ? "DUAL STACK" : v4on || v6on ? "SINGLE" : "OFFLINE"}</span>
  </div>

  <!-- 悬停展开：本机地址（设计稿 .ip-detail） -->
  <div class="ip-detail">
    <div class="line"><span class="k">v4</span><span class="v">{v4addr ?? "不可用"}</span></div>
    <div class="line"><span class="k">v6</span><span class="v">{v6addr ?? "不可用"}</span></div>
  </div>
</div>

<style>
  .widget {
    width: 100vw;
    height: 100vh;
    border-radius: 16px;
    padding: 8px 12px 6px;
    cursor: grab;
    overflow: visible;
    /* 设计稿：白玻璃 0.62 + blur(20)，窗口透明后直接悬在桌面上 */
    background: rgba(255, 255, 255, 0.62);
    backdrop-filter: blur(20px) saturate(1.8);
    -webkit-backdrop-filter: blur(20px) saturate(1.8);
    border: 1px solid var(--glass-edge);
    box-shadow: 0 10px 32px rgba(30, 40, 60, 0.16), 0 2px 6px rgba(30, 40, 60, 0.08);
    transition: transform 0.25s ease, background 0.25s ease, box-shadow 0.25s ease;
  }
  .widget:hover {
    background: rgba(255, 255, 255, 0.82);
    box-shadow: 0 14px 40px rgba(30, 40, 60, 0.2);
  }
  .widget:active { cursor: grabbing; }

  .row {
    display: flex;
    align-items: center;
    gap: 7px;
  }
  .row + .row { margin-top: 3px; }
  .icon {
    width: 13px;
    font-size: 11px;
    font-weight: 700;
    text-align: center;
  }
  .icon.dl { color: var(--accent-v4); }
  .icon.ul { color: var(--accent-v6); }
  .numv {
    font-size: 13px;
    font-weight: 700;
    font-variant-numeric: tabular-nums;
    letter-spacing: -0.3px;
    line-height: 1.15;
    min-width: 54px;
  }
  .bar {
    flex: 1;
    height: 3px;
    border-radius: 2px;
    background: rgba(0, 0, 0, 0.07);
    overflow: hidden;
  }
  .fill {
    display: block;
    height: 100%;
    border-radius: 2px;
    transition: width 0.6s ease;
  }
  .fill-dl { background: var(--accent-v4); }
  .fill-ul { background: var(--accent-v6); }

  .ip-row {
    display: flex;
    align-items: center;
    gap: 5px;
    margin-top: 6px;
    padding-top: 5px;
    border-top: 1px solid rgba(0, 0, 0, 0.06);
  }
  .ip-chip {
    display: flex;
    align-items: center;
    gap: 3px;
    font-size: 8.5px;
    font-weight: 700;
    letter-spacing: 0.3px;
    padding: 1px 5px;
    border-radius: 8px;
    background: rgba(0, 0, 0, 0.05);
    color: var(--text-tertiary);
  }
  .ip-chip.chip-on { color: var(--accent-v4); background: rgba(0, 113, 227, 0.12); }
  .ip-chip + .ip-chip.chip-on { color: var(--accent-v6); background: rgba(0, 168, 146, 0.12); }
  .pulse {
    width: 4px;
    height: 4px;
    border-radius: 50%;
    background: currentColor;
    animation: breathe 2s ease-in-out infinite;
  }
  @keyframes breathe {
    50% { opacity: 0.35; }
  }
  .dual {
    flex: 1;
    text-align: right;
    font-size: 8.5px;
    font-weight: 600;
    color: var(--text-tertiary);
    letter-spacing: 0.3px;
  }

  /* 悬停时从底部滑出：本机地址详情 */
  .ip-detail {
    position: absolute;
    top: calc(100% + 8px);
    left: 0;
    right: 0;
    border-radius: 14px;
    background: rgba(255, 255, 255, 0.85);
    backdrop-filter: blur(24px) saturate(1.8);
    -webkit-backdrop-filter: blur(24px) saturate(1.8);
    border: 1px solid var(--glass-edge);
    box-shadow: 0 10px 32px rgba(30, 40, 60, 0.16), 0 2px 6px rgba(30, 40, 60, 0.08);
    padding: 8px 12px;
    opacity: 0;
    transform: translateY(-6px);
    pointer-events: none;
    transition: opacity 0.25s ease, transform 0.25s ease;
    z-index: 11;
  }
  .widget:hover .ip-detail {
    opacity: 1;
    transform: translateY(0);
  }
  .ip-detail .line {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 10px;
    font-variant-numeric: tabular-nums;
  }
  .ip-detail .line + .line { margin-top: 4px; }
  .ip-detail .k {
    color: var(--text-tertiary);
    font-weight: 700;
    font-size: 8.5px;
    width: 20px;
  }
  .ip-detail .v {
    color: var(--text-secondary);
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
