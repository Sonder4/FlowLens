// 全局状态：Svelte 5 runes（$state / $derived），无状态管理库
// 数据来源：后端事件（traffic-tick / io-tick / capture-state）+ 命令查询

import { api, listen } from "./tauri";
import type { AdapterIo, ConnInfo, DeviceInfo, DeviceTick, HistBucket } from "./tauri";

export const SPEED_SAMPLES = 90;

export const state = $state({
  running: false,
  devices: [] as DeviceInfo[],
  activeDevice: null as string | null, // null = 全部网卡
  io: [] as AdapterIo[],
  // 实时速率环形缓冲（bytes/s）
  speedSamples: [] as { rx: number; tx: number }[],
  // 会话累计
  sessionRx: 0,
  sessionTx: 0,
  // 24 小时历史（v4/v6 × rx/tx）
  hourly: [] as HistBucket[],
  // 连接面板（各设备合并，按累计排序）
  conns: [] as ConnInfo[],
  familyFilter: null as "v4" | "v6" | null,
  errorMsg: null as string | null,
});

export function totalSpeed() {
  const filtered = state.activeDevice
    ? state.io.filter((a) => a.name === state.activeDevice)
    : state.io;
  return filtered.reduce(
    (acc, a) => ({ rx: acc.rx + a.rxSpeed, tx: acc.tx + a.txSpeed }),
    { rx: 0, tx: 0 },
  );
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
  for (const c of state.conns) {
    if (c.family === "v4") v4 += 1;
    else v6 += 1;
  }
  return { total: v4 + v6, v4, v6 };
}

export function filteredConns() {
  return state.familyFilter
    ? state.conns.filter((c) => c.family === state.familyFilter)
    : state.conns;
}

let loaded = false;

export async function initState(): Promise<void> {
  if (loaded) return;
  loaded = true;

  state.running = await api.captureRunning();
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

function onTickAccumulate(tick: DeviceTick): void {
  // 会话累计与实时样本（仅统计当前选择范围内）
  const relevant =
    !state.activeDevice || tick.device === state.activeDevice;
  if (relevant) {
    state.sessionRx += tick.totalRx;
    state.sessionTx += tick.totalTx;
    const last = state.speedSamples.at(-1);
    const lastKey = `${last?.rx ?? -1}/${last?.tx ?? -1}`;
    // 用最新一秒的速率覆盖式推进：后端每秒一 tick，直接 push
    void lastKey;
    state.speedSamples.push({ rx: tick.totalRx, tx: tick.totalTx });
    if (state.speedSamples.length > SPEED_SAMPLES) {
      state.speedSamples.shift();
    }
    state.conns = mergeConns(state.conns, tick.conns, tick.device);
  }
}

function mergeConns(
  current: ConnInfo[],
  incoming: ConnInfo[],
  _device: string,
): ConnInfo[] {
  const map = new Map<string, ConnInfo>();
  for (const c of current) map.set(c.remote, c);
  for (const c of incoming) {
    const old = map.get(c.remote);
    if (old) {
      // 取较大者：不同设备出现的同一远端视为同一连接
      map.set(c.remote, {
        ...c,
        rx: Math.max(old.rx, c.rx),
        tx: Math.max(old.tx, c.tx),
      });
    } else {
      map.set(c.remote, c);
    }
  }
  return [...map.values()].sort((a, b) => b.rx + b.tx - (a.rx + a.tx)).slice(0, 12);
}

function onTrafficTick(tick: DeviceTick): void {
  onTickAccumulate(tick);

  // 更新当前小时的内存柱（24h 图的最后一根随实时数据增长）
  const label = currentHourLabel();
  const last = state.hourly.at(-1);
  const fresh = last?.label === label;
  const bucket = fresh
    ? (state.hourly[state.hourly.length - 1] = { ...last })
    : {
        label,
        rxV4: 0,
        rxV6: 0,
        txV4: 0,
        txV6: 0,
      };
  if (!fresh) {
    state.hourly.push(bucket);
    if (state.hourly.length > 24) state.hourly.shift();
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

export async function selectDevice(device: string | null): Promise<void> {
  state.activeDevice = device;
  state.sessionRx = 0;
  state.sessionTx = 0;
  state.speedSamples.length = 0;
  state.conns = [];
  await api.startCapture(device);
}

export async function refreshHourly(): Promise<void> {
  const raw = await api.history("hourly", null);
  if (raw.length === 0 || raw.some((b) => !b || typeof b.rxV4 !== "number")) {
    console.error("hourly payload: " + JSON.stringify(raw).slice(0, 300));
  }
  state.hourly = raw;
}
