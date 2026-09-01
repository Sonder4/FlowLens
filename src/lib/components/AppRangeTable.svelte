<!-- 范围内应用流量明细：按进程累计，IPv4 / IPv6 分列，附四类筛选（系统/软件/开发/未归类） -->
<script lang="ts">
  import { fmtBytes } from "../tauri";
  import type { AppRangeRow, AppCategory } from "../tauri";

  interface Props {
    rows: AppRangeRow[];
    loading?: boolean;
  }
  let { rows, loading = false }: Props = $props();

  type Filter = AppCategory | "all";
  let filter: Filter = $state("all");

  const CATS: { key: Filter; label: string }[] = [
    { key: "all", label: "全部" },
    { key: "system", label: "系统" },
    { key: "software", label: "软件" },
    { key: "dev", label: "开发" },
    { key: "other", label: "未归类" },
  ];

  const catLabel: Record<AppCategory, string> = {
    system: "系统",
    dev: "开发",
    software: "软件",
    other: "未归类",
  };

  const filtered = $derived(
    filter === "all" ? rows : rows.filter((r) => r.category === filter),
  );

  const totals = $derived.by(() => {
    let rxV4 = 0;
    let txV4 = 0;
    let rxV6 = 0;
    let txV6 = 0;
    for (const r of filtered) {
      rxV4 += r.rxV4;
      txV4 += r.txV4;
      rxV6 += r.rxV6;
      txV6 += r.txV6;
    }
    return { rxV4, txV4, rxV6, txV6 };
  });

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

  const valOf = (r: AppRangeRow): number => {
    switch (sortKey) {
      case "v4":
        return r.rxV4 + r.txV4;
      case "v6":
        return r.rxV6 + r.txV6;
      case "rx":
        return r.rxV4 + r.rxV6;
      case "tx":
        return r.txV4 + r.txV6;
      default:
        return r.rxV4 + r.txV4 + r.rxV6 + r.txV6;
    }
  };

  const sorted = $derived.by(() => {
    const m = sortDir === "asc" ? 1 : -1;
    return [...filtered].sort((a, b) =>
      sortKey === "name"
        ? m * a.app.localeCompare(b.app)
        : m * (valOf(a) - valOf(b)),
    );
  });
</script>

<div class="table-wrap">
  <div class="chips">
    {#each CATS as c (c.key)}
      <button class="chip" class:active={filter === c.key}
              onclick={() => (filter = c.key)}>{c.label}</button>
    {/each}
  </div>
  <table>
    <thead>
      <tr>
        <th>分类</th>
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
      {#each sorted as r (r.app)}
        <tr>
          <td><span class="badge {r.category}">{catLabel[r.category]}</span></td>
          <td class="prog">{r.app}</td>
          <td class="r num v4">{fmtBytes(r.rxV4)} / {fmtBytes(r.txV4)}</td>
          <td class="r num v6">{fmtBytes(r.rxV6)} / {fmtBytes(r.txV6)}</td>
          <td class="r num rx">{fmtBytes(r.rxV4 + r.rxV6)}</td>
          <td class="r num tx">{fmtBytes(r.txV4 + r.txV6)}</td>
        </tr>
      {:else}
        <tr>
          <td colspan="6" class="empty">
            {loading ? "加载中…" : "所选范围内暂无应用流量数据"}
          </td>
        </tr>
      {/each}
    </tbody>
    {#if filtered.length > 0}
      <tfoot>
        <tr>
          <td colspan="2" class="sum">小计（{filtered.length} 个应用）</td>
          <td class="r num">{fmtBytes(totals.rxV4)} / {fmtBytes(totals.txV4)}</td>
          <td class="r num">{fmtBytes(totals.rxV6)} / {fmtBytes(totals.txV6)}</td>
          <td class="r num rx">{fmtBytes(totals.rxV4 + totals.rxV6)}</td>
          <td class="r num tx">{fmtBytes(totals.txV4 + totals.txV6)}</td>
        </tr>
      </tfoot>
    {/if}
  </table>
  <p class="note">
    点击列标题可切换排序（默认按总流量降序，再点同列切换升序）。
    分类说明：<b>系统</b> = Windows 自身（svchost 服务、系统更新、Defender 等系统进程）；
    <b>软件</b> = 已安装应用（Edge、微信等，按安装目录与产品名匹配）；
    <b>开发</b> = 开发工具链（node / npm / git / cargo / python 等产生的下载与拉取流量）；
    <b>未归类</b> = 暂时无法归属到进程的流量与未知程序。
    浏览器内下载 GitHub 资源会计入浏览器所属软件。
  </p>
</div>

<style>
  .table-wrap { width: 100%; }
  .chips {
    display: flex;
    gap: 6px;
    margin-bottom: 10px;
    flex-wrap: wrap;
  }
  .chip {
    padding: 4px 12px;
    border: none;
    border-radius: 999px;
    background: rgba(0, 0, 0, 0.05);
    color: var(--text-secondary);
    font-size: var(--fs-xs);
    font-weight: 600;
    font-family: inherit;
    cursor: pointer;
    transition: all 0.2s ease;
  }
  .chip:hover { background: rgba(255, 255, 255, 0.9); color: var(--text-primary); }
  .chip.active {
    background: #fff;
    color: var(--text-primary);
    box-shadow: 0 1px 4px rgba(30, 40, 60, 0.12);
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
  tbody tr:hover { background: rgba(255, 255, 255, 0.85); }
  .r { text-align: right; }
  .prog {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .badge {
    display: inline-block;
    padding: 1px 8px;
    border-radius: 999px;
    font-size: var(--fs-xs);
    font-weight: 600;
    color: #fff;
  }
  .badge.system { background: #6b7a90; }
  .badge.software { background: var(--accent-v4, #2f7cf6); }
  .badge.dev { background: #8e6bd6; }
  .badge.other { background: #b0b4bc; }
  .v4 { color: var(--accent-v4); }
  .v6 { color: var(--accent-v6); }
  .rx { color: var(--accent-v4); }
  .tx { color: var(--orange); }
  tfoot td {
    border-top: 1px solid rgba(0, 0, 0, 0.08);
    border-bottom: none;
    font-weight: 600;
  }
  .sum { color: var(--text-secondary); }
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
  .note b { color: var(--text-secondary); }
</style>
