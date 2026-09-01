<!-- 应用每日流量历史：读取本地 SQLite（traffic_app_day），
     仅展示单日合计 > 100MB 的应用，IPv4 / IPv6 收发分列 -->
<script lang="ts">
  import { onMount } from "svelte";
  import { api, fmtBytes } from "../tauri";
  import type { AppDayRow } from "../tauri";

  let rows: AppDayRow[] = $state([]);
  let loading = $state(true);

  const dayTotal = (r: AppDayRow): number =>
    r.rxV4 + r.txV4 + r.rxV6 + r.txV6;

  async function load(): Promise<void> {
    try {
      rows = await api.historyAppDay();
    } finally {
      loading = false;
    }
  }

  onMount(load);
</script>

<div class="table-wrap">
  <table>
    <thead>
      <tr>
        <th>日期</th>
        <th>应用 / 进程</th>
        <th class="r">IPv4 ↓/↑</th>
        <th class="r">IPv6 ↓/↑</th>
        <th class="r">当日 ↓</th>
        <th class="r">当日 ↑</th>
      </tr>
    </thead>
    <tbody>
      {#each rows as r (r.day + r.app)}
        <tr>
          <td class="day num">{r.day}</td>
          <td class="prog">{r.app}</td>
          <td class="r num v4">{fmtBytes(r.rxV4)} / {fmtBytes(r.txV4)}</td>
          <td class="r num v6">{fmtBytes(r.rxV6)} / {fmtBytes(r.txV6)}</td>
          <td class="r num rx">{fmtBytes(r.rxV4 + r.rxV6)}</td>
          <td class="r num tx">{fmtBytes(r.txV4 + r.txV6)}</td>
        </tr>
      {:else}
        <tr>
          <td colspan="6" class="empty">
            {loading
              ? "加载中…"
              : "暂无数据 — 应用单日合计流量超过 100 MB 后自动入库（IPv4 / IPv6 分列，关闭应用时也会保存）"}
          </td>
        </tr>
      {/each}
    </tbody>
  </table>
  <p class="note">
    为减少数据库体积，仅持久化单日合计（收 + 发、v4 + v6）超过 100 MB 的应用；
    未达门槛的应用不写入数据库。数据保存在本地 SQLite，重启后仍可查看。
  </p>
</div>

<style>
  .table-wrap {
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
  td {
    padding: 6px 10px 6px 0;
    border-bottom: 1px solid rgba(0, 0, 0, 0.04);
    font-size: var(--fs-sm);
  }
  tbody tr:hover { background: rgba(255, 255, 255, 0.85); }
  .r { text-align: right; }
  .day {
    color: var(--text-tertiary);
    font-variant-numeric: tabular-nums;
  }
  .prog {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .v4 { color: var(--accent-v4); }
  .v6 { color: var(--accent-v6); }
  .rx { color: var(--accent-v4); }
  .tx { color: var(--orange); }
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
