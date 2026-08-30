<script lang="ts">
  import { fmtSpeed } from "../tauri";
  import { state } from "../state.svelte";

  interface Props {
    samples: { rx: number; tx: number }[];
  }
  let { samples }: Props = $props();

  const W = 320;
  const H = 170;
  const PAD_L = 8;
  const PAD_R = 8;
  const PAD_T = 10;
  const PAD_B = 20;

  const chartW = $derived(W - PAD_L - PAD_R);
  const chartH = $derived(H - PAD_T - PAD_B);

  const maxV = $derived(Math.max(1, ...samples.map((s) => Math.max(s.rx, s.tx))));

  // 平滑贝塞尔曲线路径
  function smoothPath(values: number[]): string {
    if (values.length < 2) return "";
    const n = values.length;
    const step = chartW / (n - 1);
    const pts = values.map((v, i) => ({
      x: PAD_L + step * i,
      y: PAD_T + chartH - (v / maxV) * chartH,
    }));
    let d = `M ${pts[0].x} ${pts[0].y}`;
    for (let i = 1; i < pts.length; i++) {
      const p0 = pts[i - 1];
      const p1 = pts[i];
      const cx = (p0.x + p1.x) / 2;
      d += ` C ${cx} ${p0.y}, ${cx} ${p1.y}, ${p1.x} ${p1.y}`;
    }
    return d;
  }

  const rxPath = $derived(smoothPath(samples.map((s) => s.rx)));
  const txPath = $derived(smoothPath(samples.map((s) => s.tx)));
  const rxArea = $derived(
    rxPath
      ? `${rxPath} L ${PAD_L + chartW} ${PAD_T + chartH} L ${PAD_L} ${PAD_T + chartH} Z`
      : "",
  );

  const lastRx = $derived(samples.at(-1)?.rx ?? 0);
  const lastTx = $derived(samples.at(-1)?.tx ?? 0);
</script>

<div class="wrap">
  <div class="head">
    <span class="title">实时网速</span>
    <span class="now num">
      <span class="dot rx" />▼ {fmtSpeed(lastRx)}
      <span class="dot tx" />▲ {fmtSpeed(lastTx)}
    </span>
  </div>
  <svg viewBox="0 0 {W} {H}" class="chart">
    <defs>
      <linearGradient id="rxFill" x1="0" y1="0" x2="0" y2="1">
        <stop offset="0%" stop-color="var(--accent-v4)" stop-opacity="0.20" />
        <stop offset="100%" stop-color="var(--accent-v4)" stop-opacity="0.02" />
      </linearGradient>
    </defs>

    <line x1={PAD_L} x2={W - PAD_R} y1={PAD_T + chartH / 2} y2={PAD_T + chartH / 2}
          stroke="rgba(0,0,0,0.05)" />
    <line x1={PAD_L} x2={W - PAD_R} y1={PAD_T + chartH} y2={PAD_T + chartH}
          stroke="rgba(0,0,0,0.08)" />

    {#if rxArea}
      <path d={rxArea} fill="url(#rxFill)" />
      <path d={rxPath} fill="none" stroke="var(--accent-v4)" stroke-width="2" />
      <path d={txPath} fill="none" stroke="var(--orange)" stroke-width="1.5" opacity="0.85" />
    {:else}
      <text x={W / 2} y={H / 2} text-anchor="middle" class="placeholder">等待数据…</text>
    {/if}

    <text x={PAD_L} y={H - 5} class="tick num">-90s</text>
    <text x={W / 2} y={H - 5} text-anchor="middle" class="tick num">-45s</text>
    <text x={W - PAD_R} y={H - 5} text-anchor="end" class="tick num">现在</text>
  </svg>
</div>

<style>
  .wrap {
    width: 100%;
    height: 100%;
    display: flex;
    flex-direction: column;
  }
  .head {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    margin-bottom: 2px;
  }
  .title {
    font-size: var(--fs-lg);
    font-weight: 600;
  }
  .now {
    font-size: var(--fs-sm);
    color: var(--text-secondary);
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    display: inline-block;
  }
  .dot.rx { background: var(--accent-v4); }
  .dot.tx { background: var(--orange); }
  .chart { width: 100%; flex: 1; min-height: 0; }
  .placeholder { font-size: 12px; fill: var(--text-tertiary); }
  .tick { font-size: 10px; fill: var(--text-tertiary); }
</style>
