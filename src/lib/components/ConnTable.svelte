<script lang="ts">
  import { fmtBytes } from "../tauri";
  import type { FlowInfo } from "../tauri";

  interface Props {
    flows: FlowInfo[];
    rows?: number;
    compact?: boolean;
  }
  let { flows, rows = 6, compact = false }: Props = $props();
</script>

<div class="panel glass" class:compact>
  <div class="head">
    <span class="title">{compact ? "实时连接" : "连接详情"}</span>
    <span class="sub num">{flows.length} 条流 · 按累计排序</span>
  </div>
  <div class="table-wrap">
    <table>
      <thead>
        <tr>
          <th>应用 / 进程</th>
          <th>协议</th>
          <th>远端地址</th>
          <th class="r">下载累计</th>
          <th class="r">上传累计</th>
        </tr>
      </thead>
      <tbody>
        {#each flows.slice(0, rows) as f (f.remote + f.remotePort + f.localPort + f.proto)}
          <tr>
            <td class="prog">{f.program}</td>
            <td class="pcol">
              <span class="badge {f.family}">{f.family === "v4" ? "IPv4" : "IPv6"}</span>
              <span class="proto">{f.proto}</span>
            </td>
            <td class="addr num">{f.remote}:{f.remotePort}</td>
            <td class="r num rx">{fmtBytes(f.rx)}</td>
            <td class="r num tx">{fmtBytes(f.tx)}</td>
          </tr>
        {:else}
          <tr>
            <td colspan="5" class="empty">暂无连接数据 — 抓包运行后显示远端主机与应用</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
</div>

<style>
  .panel {
    width: 100%;
    height: 100%;
    padding: 14px 18px;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }
  .head {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    margin-bottom: 8px;
  }
  .title {
    font-size: var(--fs-lg);
    font-weight: 600;
  }
  .sub {
    font-size: var(--fs-xs);
    color: var(--text-tertiary);
  }
  .table-wrap {
    flex: 1;
    overflow: hidden;
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
  tr { transition: background 0.25s ease; }
  tbody tr:hover { background: rgba(255, 255, 255, 0.85); }
  .r { text-align: right; }
  .pcol { white-space: nowrap; }
  .proto {
    margin-left: 6px;
    font-size: var(--fs-xs);
    color: var(--text-tertiary);
  }
  .prog {
    font-weight: 600;
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
  .rx { color: var(--accent-v4); }
  .tx { color: var(--orange); }
  .empty {
    text-align: center;
    color: var(--text-tertiary);
    padding: 18px 0;
  }
</style>
