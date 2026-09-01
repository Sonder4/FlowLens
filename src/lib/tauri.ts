// Tauri command / event 封装与数据类型

export interface DeviceInfo {
  name: string;
  desc: string | null;
  addresses: string[];
  display: string;
}

export interface HistBucket {
  label: string;
  rxV4: number;
  rxV6: number;
  txV4: number;
  txV6: number;
}

// 应用每日流量历史行（仅入库单日合计 > 100MB 的应用）
export interface AppDayRow {
  day: string;
  app: string;
  rxV4: number;
  txV4: number;
  rxV6: number;
  txV6: number;
}

// 任意时间范围查询：后端按跨度自动选择 小时/天 桶
export interface RangeSeries {
  granularity: "hour" | "day";
  buckets: HistBucket[];
}

// 应用流量分类
export type AppCategory = "system" | "dev" | "software" | "other";

// 任意时间范围内单个应用的流量聚合（无门槛，附分类）
export interface AppRangeRow {
  app: string;
  category: AppCategory;
  rxV4: number;
  txV4: number;
  rxV6: number;
  txV6: number;
}

// 已安装软件目录条目（注册表 Uninstall 键）
export interface InstalledApp {
  name: string;
  publisher: string | null;
  installLocation: string | null;
}

export interface AdapterIo {
  name: string;
  rxSpeed: number;
  txSpeed: number;
  totalRx: number;
  totalTx: number;
}

export interface FlowInfo {
  device: string;
  remote: string;
  remotePort: number;
  localPort: number;
  proto: "TCP" | "UDP";
  family: "v4" | "v6";
  rx: number;
  tx: number;
  program: string;
}

export interface DeviceTick {
  device: string;
  rxV4: number;
  rxV6: number;
  txV4: number;
  txV6: number;
  totalRx: number;
  totalTx: number;
  flows: FlowInfo[];
}

export interface AdapterBinding {
  name: string;
  ipv4: boolean;
  ipv6: boolean;
}

export interface PolicyStatus {
  v6Precedence: number;
  v4Precedence: number;
  preferIpv6: boolean;
  adapters: AdapterBinding[];
  elevated: boolean;
  error: string | null;
}

async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args);
}

export const api = {
  listDevices: () => invoke<DeviceInfo[]>("list_devices"),
  startCapture: (device: string | null) => invoke<void>("start_capture", { device }),
  stopCapture: () => invoke<void>("stop_capture"),
  captureRunning: () => invoke<boolean>("capture_running"),
  setupDone: () => invoke<boolean>("setup_done"),
  ioSnapshot: () => invoke<AdapterIo[]>("io_snapshot"),
  history: (granularity: string, adapter: string | null) =>
    invoke<HistBucket[]>("history", { granularity, adapter }),
  historyAppDay: () => invoke<AppDayRow[]>("history_app_day"),
  historyRange: (since: number, until: number, adapter: string | null) =>
    invoke<RangeSeries>("history_range", { since, until, adapter }),
  historyAppRange: (since: number, until: number) =>
    invoke<AppRangeRow[]>("history_app_range", { since, until }),
  listInstalledApps: () => invoke<InstalledApp[]>("list_installed_apps"),
  knownAdapters: () => invoke<[string, string | null][]>("known_adapters"),
  popupFloatingMenu: () => invoke<void>("popup_floating_menu"),
  localAddresses: () => invoke<string[]>("local_addresses"),
  showWindow: (label: string) => invoke<void>("show_window", { label }),
  hideWindow: (label: string) => invoke<void>("hide_window", { label }),
  ipv6PolicyStatus: () => invoke<PolicyStatus>("ipv6_policy_status"),
  setIpv6Policy: (mode: string) => invoke<string>("set_ipv6_policy", { mode }),
  restartAsAdmin: () => invoke<void>("restart_as_admin"),
};

export async function listen<T>(
  event: string,
  handler: (payload: T) => void,
): Promise<() => void> {
  const { listen } = await import("@tauri-apps/api/event");
  const unlisten = await listen<T>(event, (e) => handler(e.payload));
  return unlisten;
}

// ---- 格式化工具（全站数字 tabular-nums） ----

export function fmtBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.max(0, Math.min(units.length - 1, Math.floor(Math.log2(bytes) / 10)));
  const v = bytes / 2 ** (10 * i);
  const digits = v >= 100 || i === 0 ? 0 : 1;
  return `${v.toFixed(digits)} ${units[i]}`;
}

export function fmtSpeed(bytesPerSec: number): string {
  return `${fmtBytes(bytesPerSec)}/s`;
}

export function fmtPct(part: number, total: number): string {
  if (total <= 0) return "0%";
  return `${Math.round((part * 100) / total)}%`;
}
