<script lang="ts">
  import { fmtBytes } from "../tauri";
  import type { FlowInfo } from "../tauri";

  interface Props {
    flows: FlowInfo[];
    rows?: number;
  }
  let { flows, rows = 8 }: Props = $props();

  let expanded = $state(false);

  type SortKey = "total" | "name" | "v4" | "v6" | "rx" | "tx";
  let sortKey: SortKey = $state("total");
  let sortDir: "asc" | "desc" = $state("desc");

  // 列标题可点击切换排序；↓/↑ 在标签中表示 下行/上行，排序指示用 ▼/▲
  const COLS: { key: SortKey; label: string }[] = [
    { key: "name", label: "应用 / 进程" },
    { key: "v4", label: "IPv4 ↓/↑" },
    { key: "v6", label: "IPv6 ↓/↑" },
    { key: "rx", label: "合计 ↓" },
    { key: "tx", label: "合计 ↑" },
  ];

  function toggleSort(key: SortKey) {
    if (sortKey === key) {
      sortDir = sortDir === "desc" ? "asc" : "desc";
    } else {
      sortKey = key;
      sortDir = key === "name" ? "asc" : "desc";
    }
  }

  const valOf = (a: AppStat): number => {
    switch (sortKey) {
      case "v4":
        return a.rxV4 + a.txV4;
      case "v6":
        return a.rxV6 + a.txV6;
      case "rx":
        return a.rxV4 + a.rxV6;
      case "tx":
        return a.txV4 + a.txV6;
      default:
        return a.rxV4 + a.txV4 + a.rxV6 + a.txV6;
    }
  };

  interface AppStat {
    program: string;
    rxV4: number;
    txV4: number;
    rxV6: number;
    txV6: number;
  }
  const stats = $derived.by(() => {
    const map = new Map<string, AppStat>();
    for (const f of flows) {
      const e = map.get(f.program) ?? {
        program: f.program,
        rxV4: 0,
        txV4: 0,
        rxV6: 0,
        txV6: 0,
      };
      if (f.family === "v4") {
        e.rxV4 += f.rx;
        e.txV4 += f.tx;
      } else {
        e.rxV6 += f.rx;
        e.txV6 += f.tx;
      }
      map.set(f.program, e);
    }
    const m = sortDir === "asc" ? 1 : -1;
    return [...map.values()]
      .sort((a, b) =>
        sortKey === "name"
          ? m * a.program.localeCompare(b.program)
          : m * (valOf(a) - valOf(b)),
      )
      .slice(0, rows);
  });

  // 「其他」= 建流瞬间系统端口表里查不到归属进程的流（见下方说明）
  const others = $derived(
    flows
      .filter((f) => f.program === "其他")
      .sort((a, b) => b.rx + b.tx - (a.rx + a.tx)),
  );
</script>

<div class="table-wrap">
  <table>
    <thead>
      <tr>
        {#each COLS as c (c.key)}
          <th class={c.key === "name" ? "" : "r"}>
            <button
              class="sort"
              class:active={sortKey === c.key}
              title="点击按「{c.label}」排序，再次点击切换升 / 降序"
              onclick={() => toggleSort(c.key)}
            >
              {c.label}{#if sortKey === c.key}&nbsp;<span class="arrow"
                  >{sortDir === "asc" ? "▲" : "▼"}</span
                >{/if}
            </button>
          </th>
        {/each}
      </tr>
    </thead>
    <tbody>
      {#each stats as a (a.program)}
        <tr>
          <td class="prog">
            {#if a.program === "其他" && others.length > 0}
              <button class="expand" title="展开/收起「其他」包含的连接" onclick={() => (expanded = !expanded)}>
                {expanded ? "▾" : "▸"} 其他 ({others.length})
              </button>
            {:else}
              {a.program}
            {/if}
          </td>
          <td class="r num v4">{fmtBytes(a.rxV4)} / {fmtBytes(a.txV4)}</td>
          <td class="r num v6">{fmtBytes(a.rxV6)} / {fmtBytes(a.txV6)}</td>
          <td class="r num rx">{fmtBytes(a.rxV4 + a.rxV6)}</td>
          <td class="r num tx">{fmtBytes(a.txV4 + a.txV6)}</td>
        </tr>
        {#if a.program === "其他" && expanded}
          {#each others.slice(0, 8) as f (f.device + f.remote + f.remotePort + f.localPort + f.proto)}
            <tr class="detail">
              <td class="dim">{f.proto} · {f.family === "v4" ? "IPv4" : "IPv6"} · {f.device}</td>
              <td class="addr num" colspan="2">{f.remote}:{f.remotePort}</td>
              <td class="r num rx">{fmtBytes(f.rx)}</td>
              <td class="r num tx">{fmtBytes(f.tx)}</td>
            </tr>
          {/each}
          {#if others.length > 8}
            <tr class="detail">
              <td colspan="5" class="dim">…等 {others.length} 条未归属流，完整列表见连接详情表</td>
            </tr>
          {/if}
        {/if}
      {:else}
        <tr>
          <td colspan="5" class="empty">暂无应用数据 — 抓包运行后按进程聚合显示 IPv4 / IPv6 流量</td>
        </tr>
      {/each}
    </tbody>
  </table>
  <p class="note">
    点击列标题可切换排序（默认按总流量降序，再点同列切换升序）。
    「其他」= 建立统计瞬间无法通过系统端口表归属到进程的流量：多为已关闭的 UDP 短连接、
    系统内核级流量，或刚建立尚未注册端口的连接（后台每秒重试归属，归属成功会自动改名）。
    svchost 承载的系统服务已归属到具体服务名（svchost:服务名）。
  </p>
</div>

<style>
  .table-wrap {
    /* 页面级滚动：不设内部滚动，表格按内容完整撑开 */
    flex: none;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    table-layout: fixed;
  }
  th {
    text-align: left;
    font-size: var(--fs-xs);
    color: var(--text-tertiary);
    font-weight: 500;
    padding: 4px 10px 6px 0;
    border-bottom: 1px solid rgba(0, 0, 0, 0.06);
  }
  .sort {
    border: none;
    background: transparent;
    font: inherit;
    color: inherit;
    cursor: pointer;
    padding: 0;
    transition: color 0.15s ease;
  }
  .sort:hover { color: var(--text-primary); }
  .sort.active { color: var(--text-secondary); font-weight: 600; }
  .sort .arrow { color: var(--accent-v4); font-size: 0.85em; }
  td {
    padding: 6px 10px 6px 0;
    border-bottom: 1px solid rgba(0, 0, 0, 0.04);
    font-size: var(--fs-sm);
  }
  tr { transition: background 0.25s ease; }
  tbody tr:hover { background: rgba(255, 255, 255, 0.85); }
  .r { text-align: right; }
  .prog {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .expand {
    border: none;
    background: transparent;
    font: inherit;
    font-weight: 600;
    color: inherit;
    cursor: pointer;
    padding: 0;
  }
  .expand:hover { color: var(--accent-v4); }
  .v4 { color: var(--accent-v4); }
  .v6 { color: var(--accent-v6); }
  .rx { color: var(--accent-v4); }
  .tx { color: var(--orange); }
  .detail td {
    font-size: var(--fs-xs);
    color: var(--text-secondary);
    border-bottom: none;
    padding-top: 2px;
    padding-bottom: 2px;
  }
  .dim {
    color: var(--text-tertiary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .addr {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-primary);
  }
  .empty {
    text-align: center;
    color: var(--text-tertiary);
    padding: 18px 0;
  }
  .note {
    margin-top: 8px;
    font-size: var(--fs-xs);
    color: var(--text-tertiary);
    line-height: 1.6;
  }
</style>
