// 全局状态：Svelte 5 runes（$state / $derived），无状态管理库
// 数据来源：后端事件（traffic-tick / io-tick / capture-state）+ 命令查询
//
// 切换网卡 = 纯前端显示过滤（后端始终捕获全部有地址网卡）：
// 每台设备的累计与采样独立保存，切换即时显示对应设备的完整数据。

import { api, listen } from "./tauri";
import type { AdapterIo, DeviceInfo, DeviceTick, FlowInfo, HistBucket } from "./tauri";

export const SPEED_SAMPLES = 90;

export const state = $state({
  running: false,
  devices: [] as DeviceInfo[],
  activeDevice: null as string | null, // null = 全部网卡
  io: [] as AdapterIo[],
  // 实时速率采样（bytes/s）："全部网卡"合并 + 每网卡独立
  samplesAll: [] as { rx: number; tx: number }[],
  samplesByDevice: {} as Record<string, { rx: number; tx: number }[]>,
  // 每设备会话累计
  deviceTotals: {} as Record<string, { rx: number; tx: number }>,
  // 24 小时历史（v4/v6 × rx/tx），始终为全部网卡
  hourly: [] as HistBucket[],
  // 流表（各设备合并，带 device 标记，按累计排序）
  flows: [] as FlowInfo[],
  familyFilter: null as "v4" | "v6" | null,
  errorMsg: null as string | null,
});

// 当前显示范围的实时速率（来自抓包采样：每秒字节 = B/s）
export function displaySpeed() {
  const last = displaySamples().at(-1);
  return { rx: last?.rx ?? 0, tx: last?.tx ?? 0 };
}

// 当前显示范围的速率采样序列
export function displaySamples() {
  return state.activeDevice
    ? (state.samplesByDevice[state.activeDevice] ?? [])
    : state.samplesAll;
}

// 当前显示范围的会话累计
export function sessionTotals() {
  if (!state.activeDevice) {
    let rx = 0;
    let tx = 0;
    for (const v of Object.values(state.deviceTotals)) {
      rx += v.rx;
      tx += v.tx;
    }
    return { rx, tx };
  }
  return state.deviceTotals[state.activeDevice] ?? { rx: 0, tx: 0 };
}

export function v6Share() {
  let v4 = 0;
  let v6 = 0;
  for (const b of state.hourly) {
    v4 += b.rxV4 + b.txV4;
    v6 += b.rxV6 + b.txV6;
  }
  const total = v4 + v6;
  return { v4, v6, total, pct: total > 0 ? Math.round((v6 * 100) / total) : 0 };
}

export function activeConnCount() {
  let v4 = 0;
  let v6 = 0;
  for (const f of filteredFlows()) {
    if (f.family === "v4") v4 += 1;
    else v6 += 1;
  }
  return { total: v4 + v6, v4, v6 };
}

export function filteredFlows() {
  return state.flows.filter(
    (f) =>
      (!state.familyFilter || f.family === state.familyFilter) &&
      (!state.activeDevice || f.device === state.activeDevice),
  );
}

/// 按进程/应用聚合（主页 TOP 卡片与检查页数据源），尊重当前设备过滤
export function appTop() {
  const map = new Map<string, { program: string; rx: number; tx: number; v4: boolean; v6: boolean }>();
  for (const f of filteredFlows()) {
    const key = f.program;
    const e = map.get(key) ?? { program: key, rx: 0, tx: 0, v4: false, v6: false };
    e.rx += f.rx;
    e.tx += f.tx;
    if (f.family === "v4") e.v4 = true;
    else e.v6 = true;
    map.set(key, e);
  }
  return [...map.values()].sort((a, b) => b.rx + b.tx - (a.rx + a.tx));
}

export function mergeFlows(current: FlowInfo[], incoming: FlowInfo[]): FlowInfo[] {
  const map = new Map<string, FlowInfo>();
  for (const f of current) map.set(`${f.device}:${f.remote}:${f.remotePort}:${f.localPort}:${f.proto}`, f);
  for (const f of incoming) {
    const key = `${f.device}:${f.remote}:${f.remotePort}:${f.localPort}:${f.proto}`;
    const old = map.get(key);
    if (old) {
      map.set(key, { ...f, rx: Math.max(old.rx, f.rx), tx: Math.max(old.tx, f.tx) });
    } else {
      map.set(key, f);
    }
  }
  return [...map.values()].sort((a, b) => b.rx + b.tx - (a.rx + a.tx)).slice(0, 24);
}

let loaded = false;

const sleepMs = (ms: number): Promise<void> =>
  new Promise((r) => setTimeout(r, ms));

export async function initState(): Promise<void> {
  if (loaded) return;
  loaded = true;

  // 后端 setup（历史库/捕获引擎）在窗口加载后才完成：等待其就绪再查询，
  // 否则查询会拿到空数据且错过 capture-state 事件
  for (let i = 0; i < 50; i++) {
    if (await api.setupDone()) break;
    await sleepMs(200);
  }

  state.running = await api.captureRunning();
  liveHour = { label: currentHourLabel(), rxV4: 0, rxV6: 0, txV4: 0, txV6: 0 };
  state.io = await api.ioSnapshot();
  state.hourly = await api.history("hourly", null);

  await listen<DeviceTick>("traffic-tick", onTrafficTick);
  await listen<AdapterIo[]>("io-tick", onIoTick);
  await listen<{ running: boolean }>("capture-state", (p) => {
    state.running = p.running;
  });
  await listen<{ device: string; message: string }>("capture-error", (p) => {
    state.errorMsg = `${p.device}: ${p.message}`;
  });
}

// 跨设备同秒合并缓冲（"全部网卡"视图的采样）
let secondBuf = { sec: 0, rx: 0, tx: 0 };

function onTickAccumulate(tick: DeviceTick): void {
  // 每设备累计与采样：始终累积，切换零延迟
  const totals = (state.deviceTotals[tick.device] ??= { rx: 0, tx: 0 });
  totals.rx += tick.totalRx;
  totals.tx += tick.totalTx;
  const buf = (state.samplesByDevice[tick.device] ??= []);
  buf.push({ rx: tick.totalRx, tx: tick.totalTx });
  if (buf.length > SPEED_SAMPLES) buf.shift();

  // "全部网卡"合并采样：同一秒的多个设备 tick 求和为一个点
  const sec = Math.floor(Date.now() / 1000);
  if (secondBuf.sec !== sec) {
    if (secondBuf.sec !== 0) {
      state.samplesAll.push({ rx: secondBuf.rx, tx: secondBuf.tx });
      if (state.samplesAll.length > SPEED_SAMPLES) state.samplesAll.shift();
    }
    secondBuf = { sec, rx: 0, tx: 0 };
  }
  secondBuf.rx += tick.totalRx;
  secondBuf.tx += tick.totalTx;

  // 流表合并（打上设备标记，供切换过滤）
  state.flows = mergeFlows(
    state.flows,
    tick.flows.map((f) => ({ ...f, device: tick.device })),
  );
}

// 当前小时以内存为准：后端每 60s 才落盘一次，定时刷新会用旧数据覆盖当前桶
let liveHour = { label: "", rxV4: 0, rxV6: 0, txV4: 0, txV6: 0 };

function onTrafficTick(tick: DeviceTick): void {
  onTickAccumulate(tick);

  const label = currentHourLabel();
  if (liveHour.label !== label) {
    liveHour = { label, rxV4: 0, rxV6: 0, txV4: 0, txV6: 0 };
    state.hourly = state.hourly.filter((b) => b.label !== label);
    state.hourly.push({ ...liveHour });
    if (state.hourly.length > 24) state.hourly.shift();
  }
  liveHour.rxV4 += tick.rxV4;
  liveHour.rxV6 += tick.rxV6;
  liveHour.txV4 += tick.txV4;
  liveHour.txV6 += tick.txV6;
  // 直接改数组元素的响应式字段，确保图表柱随实时数据增长
  const last = state.hourly.at(-1);
  if (last && last.label === label) {
    last.rxV4 = liveHour.rxV4;
    last.rxV6 = liveHour.rxV6;
    last.txV4 = liveHour.txV4;
    last.txV6 = liveHour.txV6;
  }
}

export function currentHourLabel(): string {
  const d = new Date();
  const p = (n: number): string => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:00`;
}

function onIoTick(snapshot: AdapterIo[]): void {
  state.io = snapshot;
}

export function selectDevice(device: string | null): void {
  state.activeDevice = device;
}

export async function refreshHourly(): Promise<void> {
  const raw = await api.history("hourly", null);
  // 当前小时用内存实时桶（后端 60s 落盘，DB 里当前小时不完整）
  const merged = [...raw.filter((b) => b.label !== liveHour.label)];
  merged.push({ ...liveHour });
  if (merged.length > 24) merged.shift();
  state.hourly = merged;
}
