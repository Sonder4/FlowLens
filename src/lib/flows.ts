// 流表合并（纯逻辑，无 Svelte runes 依赖，便于单元测试）：
// 后端 tick 每秒发送按会话累计排序的 top-24 流。
// 这里按"最后出现时间"维护全部近期活跃流，超过 TTL 未出现的流视为已结束并剔除——
// 否则已结束/掉出后端榜单的流会以冻结计数永久占据 top-24，
// 长周期运行后陈旧大流量连接会掩盖当前活跃流。

import type { FlowInfo } from "./tauri";

const FLOW_TTL_MS = 5000;

const flowMap = new Map<string, FlowInfo>();
const flowSeen = new Map<string, number>();

export function flowKey(f: FlowInfo): string {
  return `${f.device}:${f.remote}:${f.remotePort}:${f.localPort}:${f.proto}`;
}

export function mergeFlows(incoming: FlowInfo[]): FlowInfo[] {
  const now = Date.now();
  for (const f of incoming) {
    const key = flowKey(f);
    flowMap.set(key, f);
    flowSeen.set(key, now);
  }
  // 活跃流数量有限（几秒内出现过的流），每秒全量清理开销可忽略
  for (const [key, seen] of flowSeen) {
    if (now - seen > FLOW_TTL_MS) {
      flowMap.delete(key);
      flowSeen.delete(key);
    }
  }
  return [...flowMap.values()]
    .sort((a, b) => b.rx + b.tx - (a.rx + a.tx))
    .slice(0, 24);
}
