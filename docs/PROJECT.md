# FlowLens 项目文档

> 本机网络流量监控工具 · 桌面常驻 · 长周期运行
> 文档版本：2026-09-01

## 1. 项目背景与用户需求

FlowLens 是一款 Windows 本机流量监控工具，源于"基于 Sniffnet 二次开发本机流量监控工具"的原始规划，实际演进为独立实现（Tauri 2 自建架构，保留 Sniffnet 式 pcap 抓包思路）。

### 1.1 原始需求（已全部落地）
- 监控本机 WiFi / 有线 / VPN 网卡流量，按 **网卡 × IPv4/IPv6 × 收/发方向** 区分统计
- **小时 / 日 / 月** 历史流量查询，SQLite 持久化，重启不丢
- 多网卡同时监控；桌面应用 + 悬浮窗常驻；低资源占用；液态玻璃（liquid glass）风格 UI
- 可长期长周期运行，无卡顿、无内存增长、无持续 CPU 开销
- 打包分发（NSIS 安装包 + 便携版 exe）

### 1.2 性能与长时运行优化需求（已落地）
- 抓包热路径零每包分配：进程名建流时解析一次（`Arc<str>` 复用），历史计数按秒批量聚合落库
- 端口进程表复用 `System` 实例 + 最小化增量刷新（原每 3s 全量重建）
- 前端陈旧流 TTL 剔除（>5s 未出现的连接流不再以冻结计数占据实时连接榜）
- 长时运行压力验证：27 分钟采样内存平稳（~60 MB）、平均 CPU ~1.4%（含 12 Mbit/s 下载突发）

### 1.3 应用流量统计需求（已落地）
- 应用每日流量持久化：单日合计（v4+v6、收+发）超过 **100 MB** 的应用入库（原 1GB，后按需求下调）
- 应用流量按 IPv4 / IPv6 收发分列展示

### 1.4 新增需求（2026-09-01，已落地）
1. **任意时间范围查询**：原"24 小时流量"升级为可选时间范围（预设 + 自定义起止），并给出范围内各应用的 IPv4/IPv6 数据量
2. **流量归类到已安装软件**：枚举系统已安装软件，将流量对应到软件，分四类展示——
   - **系统流量**：Windows 自身（系统更新、日志、Defender、svchost 服务等）
   - **软件流量**：用户安装软件与系统自带主要软件（Edge、微信等）
   - **开发流量**：npm / git / cargo / python 等开发工具链产生的下载与拉取流量
   - **未归类**：无法归属进程的流量与未知程序（兜底类）
3. **项目文档**：即本文档

## 2. 功能清单

| 模块 | 功能 |
|---|---|
| 仪表盘 | 实时网速曲线（90s 采样）、24 小时流量柱状图（实时增长）、应用流量表（进程归因、IPv4/IPv6 分列、「其他」可展开）、实时连接表（24 条流） |
| 顶部工具栏 | 网卡分段切换（全部/Wi-Fi/以太网/VPN/…，前端显示过滤）、IPv4/IPv6 过滤、悬浮窗唤起、抓包启停 |
| 连接详情 | 应用流量明细 + 全部活跃连接逐条列表 |
| 历史记录 | **任意时间范围**（今天/昨天/近24小时/近7天/近30天/本月/自定义）流量趋势图（自动小时/天粒度）；范围内**应用流量明细**（四分类徽标 + 筛选 + 小计）；**四类流量汇总卡**；应用每日流量永久汇总表（>100MB 门槛） |
| 设置 | 捕获范围、IP 协议策略（v4/v6 优先级，支持 UAC 提权 netsh）、窗口显示、数据说明 |
| 悬浮窗 | 常驻迷你网速窗（置顶、跳过任务栏、原生右键菜单） |
| 系统集成 | NSIS 安装包 / 便携版；数据存 `%APPDATA%\flowlens`；开机抓包自动恢复 |

## 3. 技术栈

| 层 | 技术 |
|---|---|
| 框架 | Tauri 2（Rust 后端 + WebView2 前端，多窗口：main/floating/settings） |
| 前端 | Svelte 5（runes：`$state`/`$derived`/`$props`）+ TypeScript + Vite；图表为自绘 SVG，无第三方图表库 |
| 抓包 | pcap（Npcap，WinPcap 兼容模式）+ etherparse 0.21 解析 |
| 归因 | Windows IP Helper（`GetExtendedTcp/UdpTable` 端口→PID）+ sysinfo 0.39（PID→进程名）+ SCM 枚举（svchost→服务名） |
| 持久化 | rusqlite（bundled SQLite，WAL 模式，busy_timeout 5s） |
| 软件目录 | winreg 0.55（Uninstall 注册表键枚举） |
| I/O 计数 | sysinfo Networks（io-tick 每秒事件，免管理员可用） |
| 打包 | NSIS（tauri bundle）；winreg/windows crate；release + lto + strip |

## 4. 软件架构

### 4.1 模块职责（src-tauri/src/）

```
lib.rs               Tauri 命令层（18 个命令）+ setup 装配 + 退出落盘
capture.rs           抓包引擎：每网卡一个线程；包→FlowKey 归流→程序归因
                     →app_cur 秒级聚合桶；advance_second 每秒批量落历史 + 发 traffic-tick
port_map.rs          端口→PID→进程名映射（3s 增量刷新）；svchost 服务名解析；
                     对外仅 program_for(is_tcp, is_v4, local_port)
adapter_io.rs        sysinfo 网卡计数器→每秒 io-tick 事件（速率+总量）
traffic_history/
  types.rs           BucketKey/AppHourKey/Agg/Row/RangeSeries/Granularity 等共享类型
  store.rs           SQLite 存储与查询（见 4.3）
software.rs          已安装软件目录（注册表枚举，启动加载+每日刷新）
                     + categorize() 四分类（查询时计算，不落库）
ip_policy.rs         IPv6/IPv4 前缀策略（netsh，UAC 提权）
```

### 4.2 数据流

```
pcap 包 → 解析(etherparse) → 方向/族判定
  ├─ 流表 FlowStat（会话累计，用于实时连接表，>256 LRU 淘汰）
  └─ app_cur: HashMap<AppKey{program, family}, (rx,tx)>   ← 抓包循环只做内存累加
        ↓ advance_second（每秒）
  traffic_history::record ×4（网卡×族×方向 秒级总量）
  traffic_history::record_app_totals（逐 AppKey）
        ↓ flusher 线程（每 60s）+ 退出时 flush_now
  SQLite：traffic_minute ←内存桶（累加 upsert）
          traffic_day / traffic_month ←由源表 REPLACE 幂等 rollup（近2天/近1月）
          traffic_app_hour ←app_live 内存桶（小时键，无门槛全量）
          traffic_app_day ←由 app_hour rollup（近2天）+ 100MB 门槛清理（仅过去日期）
  前端：traffic-tick（每设备每秒，含 flows top24）/ io-tick / capture-state
```

### 4.3 数据库 Schema（`%APPDATA%\flowlens\traffic_history.db`）

| 表 | 主键 | 字段 | 说明 |
|---|---|---|---|
| traffic_minute | (adapter, family, dir, ts) | bytes, pkts | ts=分钟取整 UTC 秒；**保留 90 天** |
| traffic_day | (bucket, adapter, family, dir) | bytes, pkts | bucket=本地日期；**永久** |
| traffic_month | (bucket, adapter, family, dir) | bytes, pkts | bucket=`YYYY-MM`；永久 |
| traffic_app_hour | (hour, app, family) | rx_bytes, tx_bytes | hour=`YYYY-MM-DD HH:00`；**无门槛全量**；保留 90 天 |
| traffic_app_day | (day, app, family) | rx_bytes, tx_bytes | 永久汇总；仅保留单日合计 ≥100MB 的应用 |
| adapters | name | desc, first_seen | 预留（当前未写入） |

### 4.4 查询设计

- `history(granularity, adapter)`：固定窗口（小时=近24h 分钟表 / 日=近30天 / 月=全部），CASE pivot 出 rx/tx × v4/v6 四列
- `history_range(since, until, adapter) → {granularity, buckets}`：**跨度 ≤48h 且起点在分钟保留期内 → 分钟表按小时桶；否则 → day 表按天桶**（超期范围自动降级为天粒度）
- `history_app_range(since, until) → [{app, category, rxV4, txV4, rxV6, txV6}]`：聚合 traffic_app_hour（无门槛）+ 合并未落盘内存桶，按合计降序；分类由 `software::categorize` 在命令层附加
- `history_app_day()`：永久天级汇总（含未落盘内存桶合并 + 100MB 门槛过滤）

### 4.5 流量分类规则（software.rs，查询时计算）

优先级从上到下，命中即返回：

1. `其他` → **other**
2. `svchost:服务名` 前缀 或 进程名 ∈ 系统进程清单（system/smss/lsass/dwm/MsMpEng/TrustedInstaller/SearchIndexer/MoUsoCoreWorker 等约 45 项）→ **system**
3. 进程名 ∈ 开发工具清单（node/npm/yarn/git/cargo/python/pip/conda/go/docker/dotnet/java/code/winget 等约 45 项）→ **dev**
4. 进程名 ∈ 知名软件映射（msedge/chrome/qq/weixin/feishu/obsidian/ugreen nas 等 27 项）→ **software**
5. 与已安装软件目录匹配：安装目录含同名 exe / 目录名一致 / 产品名前缀互匹配（长度护栏）→ **software**
6. 兜底 → **other**

分类不落库（查询时计算），规则可随版本演进无需数据库迁移。

## 5. UI 设计

- **风格**：液态玻璃（acrylic 材质 + 半透明卡片 + 毛玻璃 hover），浅色主题，CSS 变量统一色板（`--accent-v4` 蓝 / `--accent-v6` 青 / `--orange` 上传 / `--green` 运行态）
- **布局**：单页 + 侧边栏视图切换（dash/inspect/history/settings），侧边栏可收起（58px 图标栏 ↔ 220px 全宽，网格轨道插值动画）；主区域整页滚动
- **历史页结构**（自上而下）：范围选择条（预设 chips + 自定义起止输入）→ 四类流量汇总卡 → 趋势图（自绘 SVG 柱状，v4/v6 堆叠，悬停 tooltip，刻度按粒度自适应）→ 应用明细表（分类徽标 + 五档筛选 + 小计行）→ 应用每日流量表
- **徽标配色**：系统=灰蓝 / 软件=蓝 / 开发=紫 / 未归类=浅灰
- **三窗口**：主面板（dashboard.html）、悬浮窗（floating.html，150×76 置顶）、设置窗口（settings.html，已内嵌为主窗口视图，保留独立入口）

## 6. 打包与部署

- `npx tauri build` → release 便携 exe + `bundle/nsis/FlowLens_0.1.0_x64-setup.exe`
- 交付目录：FlowLens.exe（便携版）+ 安装包 + 使用说明.md
- NSIS 工具链需 GitHub 下载（tauri 内置超时短，必要时手动下载至 `%LOCALAPPDATA%\tauri\NSIS`，插件 DLL 需 SHA1 校验 `75197FEE…`）
- 源码仓库：github.com/Sonder4/FlowLens（MIT 开源）

## 7. 已知限制与后续方向

1. **进程名归因的边界**：浏览器内下载 GitHub 资源会计入浏览器所属软件；同一 exe 不同用途无法区分。如需精确到安装产品，可后续在 port_map 增加低频 exe 路径解析（保持 3s 热路径轻量）
2. **超 90 天的应用明细**只有 ≥100MB 的天级汇总（小时明细随保留期清理）；总流量天/月数据永久保留
3. 分类规则为静态清单，未匹配的新软件进入"未归类"；可持续补充清单或引入用户自定义规则
4. adapters 表为预留死表；known_adapters 命令未被前端使用
5. 范围查询读 DB（60s 落盘周期），当前小时的实时数据由仪表盘的 liveHour 机制覆盖，历史页暂不合并实时内存桶总量（应用明细已合并）
