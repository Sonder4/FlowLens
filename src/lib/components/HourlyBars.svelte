<script lang="ts">
  import { fmtBytes } from "../tauri";
  import type { HistBucket } from "../tauri";

  interface Props {
    data: HistBucket[];
  }
  let { data }: Props = $props();

  const W = 640;
  const H = 220;
  const PAD_L = 46;
  const PAD_R = 8;
  const PAD_T = 10;
  const PAD_B = 22;

  const chartW = $derived(W - PAD_L - PAD_R);
  const chartH = $derived(H - PAD_T - PAD_B);

  const maxTotal = $derived.by(() => {
    const values = data.map((b) =>
      Number(b.rxV4) + Number(b.rxV6) + Number(b.txV4) + Number(b.txV6),
    );
    const finite = values.filter((v) => Number.isFinite(v));
    return finite.length > 0 ? Math.max(1, ...finite) : 1;
  });

  const niceMax = $derived.by(() => {
    const raw = maxTotal;
    const mag = 10 ** Math.floor(Math.log10(raw));
    return Math.ceil(raw / mag) * mag;
  });

  const yTicks = $derived([0, 0.25, 0.5, 0.75, 1].map((f) => f * niceMax));

  const bars = $derived.by(() => {
    const n = Math.max(1, data.length);
    const slot = chartW / n;
    const barW = Math.min(26, slot * 0.55);
    return data.map((b, i) => {
      const x = PAD_L + slot * i + (slot - barW) / 2;
      const total = b.rxV4 + b.rxV6 + b.txV4 + b.txV6;
      const h = (total / niceMax) * chartH;
      const hV4 = ((b.rxV4 + b.txV4) / niceMax) * chartH;
      return {
        x,
        barW,
        y: PAD_T + chartH - h,
        h,
        hV4,
        total,
        label: b.label,
        // 悬停提示用
        rxV4: b.rxV4, rxV6: b.rxV6, txV4: b.txV4, txV6: b.txV6,
      };
    });
  });

  const xTicks = $derived.by(() => {
    if (data.length === 0) return [];
    const picks = [0, Math.floor((data.length - 1) / 3), Math.floor(((data.length - 1) * 2) / 3), data.length - 1];
    return [...new Set(picks)].map((i) => ({
      x: PAD_L + (chartW / Math.max(1, data.length)) * i + (chartW / Math.max(1, data.length)) / 2,
      label: data[i]?.label.slice(11, 16) ?? "",
    }));
  });

  let hover: number | null = $state(null);
</script>

<div class="wrap">
  <div class="legend">
    <span class="item"><i class="sw v4" />IPv4</span>
    <span class="item"><i class="sw v6" />IPv6</span>
  </div>
  <svg viewBox="0 0 {W} {H}" class="chart">
    {#each yTicks as t (t)}
      <line x1={PAD_L} x2={W - PAD_R} y1={PAD_T + chartH - (t / niceMax) * chartH}
            y2={PAD_T + chartH - (t / niceMax) * chartH}
            stroke="rgba(0,0,0,0.05)" stroke-width="1" />
      <text x={PAD_L - 6} y={PAD_T + chartH - (t / niceMax) * chartH + 3.5}
            text-anchor="end" class="tick num">{fmtBytes(t)}</text>
    {/each}

    {#each bars as b, i (b.label + String(i))}
      <g
        class="bar-group"
        onmouseenter={() => (hover = i)}
        onmouseleave={() => (hover = null)}
      >
        <rect x={b.x - 2} y={PAD_T} width={b.barW + 4} height={chartH} fill="transparent" />
        <rect x={b.x} y={b.y} width={b.barW} height={b.hV4} rx="3"
              class="seg v4" opacity={hover === null || hover === i ? 1 : 0.45} />
        <rect x={b.x} y={b.y + b.hV4} width={b.barW} height={Math.max(0, b.h - b.hV4)} rx="3"
              class="seg v6" opacity={hover === null || hover === i ? 0.9 : 0.4} />
      </g>
    {/each}

    {#each xTicks as t (t.x)}
      <text x={t.x} y={H - 6} text-anchor="middle" class="tick num">{t.label}</text>
    {/each}

    {#if hover !== null && bars[hover]}
      {@const b = bars[hover]}
      <g class="tooltip">
        <rect x={Math.min(b.x - 40, W - 150)} y={PAD_T + 4} width="142" height="76" rx="8"
              fill="rgba(255,255,255,0.92)" stroke="rgba(0,0,0,0.06)" />
        <text x={Math.min(b.x - 40, W - 150) + 10} y={PAD_T + 22} class="tt">{b.label}</text>
        <text x={Math.min(b.x - 40, W - 150) + 10} y={PAD_T + 40} class="tt">
          <tspan class="dot v4">●</tspan> IPv4 <tspan class="num">{fmtBytes(b.rxV4 + b.txV4)}</tspan>
        </text>
        <text x={Math.min(b.x - 40, W - 150) + 10} y={PAD_T + 58} class="tt">
          <tspan class="dot v6">●</tspan> IPv6 <tspan class="num">{fmtBytes(b.rxV6 + b.txV6)}</tspan>
        </text>
        <text x={Math.min(b.x - 40, W - 150) + 10} y={PAD_T + 74} class="tt">
          合计 <tspan class="num">{fmtBytes(b.total)}</tspan>
        </text>
      </g>
    {/if}
  </svg>
</div>

<style>
  .wrap {
    width: 100%;
    height: 100%;
    display: flex;
    flex-direction: column;
  }
  .legend {
    display: flex;
    justify-content: flex-end;
    gap: 12px;
    font-size: var(--fs-xs);
    color: var(--text-secondary);
    margin-bottom: 2px;
  }
  .item { display: inline-flex; align-items: center; gap: 5px; }
  .sw { width: 9px; height: 9px; border-radius: 3px; display: inline-block; }
  .sw.v4 { background: var(--accent-v4); }
  .sw.v6 { background: var(--accent-v6); }
  .chart { width: 100%; flex: 1; min-height: 0; }
  .bar-group { cursor: pointer; }
  .seg.v4 { fill: var(--accent-v4); transition: opacity 0.25s ease; }
  .seg.v6 { fill: var(--accent-v6); transition: opacity 0.25s ease; }
  .tick {
    font-size: 10px;
    fill: var(--text-tertiary);
  }
  .tt { font-size: 11px; fill: var(--text-primary); }
  .dot.v4 { fill: var(--accent-v4); }
  .dot.v6 { fill: var(--accent-v6); }
</style>
