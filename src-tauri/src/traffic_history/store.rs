//! SQLite-backed persistent store for per-adapter, per-family (IPv4/IPv6),
//! per-direction traffic, aggregated in minute buckets.
//!
//! Data flow: capture threads `record()` into an in-memory map, a background
//! flusher drains it into `traffic_minute` every 60 seconds, and daily/monthly
//! rollup tables are recomputed from their source tables on every flush so
//! that day/month history survives minute-level retention pruning.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::types::Value;
use rusqlite::{Connection, OpenFlags};

use super::types::{
    AppAgg, AppDayRow, AppHourKey, AppUsageRow,
    BucketAgg, BucketKey, Dir, Family, Granularity, HistBucket, RangeSeries,
};

/// Minute-level details are kept for this many days; day/month rollups are kept forever.
const RETENTION_MINUTE_DAYS: i64 = 90;

/// Hour-level per-app details share the minute table's retention window.
const RETENTION_APP_HOUR_DAYS: i64 = 90;

/// 跨度不超过该值的时间范围按小时粒度出桶（更大跨度按天）。
const HOUR_BUCKET_MAX_SPAN_SECS: i64 = 48 * 3600;

/// 应用每日流量入库门槛：单日（v4+v6、收+发）合计超过 100 MB 才持久化。
/// 低于门槛的行在每次落盘时清理（含跨天的历史残留），显著压缩数据库体积。
pub const APP_DAY_THRESHOLD_BYTES: u64 = 100_000_000;

/// Flush cadence of the background flusher thread, in seconds.
pub const FLUSH_INTERVAL_SECS: u64 = 60;

pub struct HistoryStore {
    conn: Mutex<Connection>,
    live: Mutex<HashMap<BucketKey, BucketAgg>>,
    app_live: Mutex<HashMap<AppHourKey, AppAgg>>,
}

impl HistoryStore {
    /// Opens (and creates, if needed) the store database at the given path.
    /// Use `Connection::open_in_memory` semantics via the special path `:memory:`.
    pub fn open(path: &Path) -> Result<Self, rusqlite::Error> {
        let conn = if path == Path::new(":memory:") {
            Connection::open_in_memory()?
        } else {
            Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
            )?
        };
        Self::with_connection(conn)
    }

    /// Wraps an existing connection; used by tests with in-memory databases.
    pub fn with_connection(conn: Connection) -> Result<Self, rusqlite::Error> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::create_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            live: Mutex::new(HashMap::new()),
            app_live: Mutex::new(HashMap::new()),
        })
    }

    fn create_schema(conn: &Connection) -> Result<(), rusqlite::Error> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS traffic_minute (
                adapter TEXT NOT NULL,
                family  TEXT NOT NULL,
                dir     TEXT NOT NULL,
                ts      INTEGER NOT NULL,
                bytes   INTEGER NOT NULL,
                pkts    INTEGER NOT NULL,
                PRIMARY KEY (adapter, family, dir, ts)
            );
            CREATE INDEX IF NOT EXISTS idx_minute_ts ON traffic_minute (ts);
            CREATE TABLE IF NOT EXISTS traffic_day (
                bucket  TEXT NOT NULL,
                adapter TEXT NOT NULL,
                family  TEXT NOT NULL,
                dir     TEXT NOT NULL,
                bytes   INTEGER NOT NULL,
                pkts    INTEGER NOT NULL,
                PRIMARY KEY (bucket, adapter, family, dir)
            );
            CREATE TABLE IF NOT EXISTS traffic_month (
                bucket  TEXT NOT NULL,
                adapter TEXT NOT NULL,
                family  TEXT NOT NULL,
                dir     TEXT NOT NULL,
                bytes   INTEGER NOT NULL,
                pkts    INTEGER NOT NULL,
                PRIMARY KEY (bucket, adapter, family, dir)
            );
            CREATE TABLE IF NOT EXISTS adapters (
                name       TEXT PRIMARY KEY,
                desc       TEXT,
                first_seen INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS traffic_app_day (
                day      TEXT NOT NULL,
                app      TEXT NOT NULL,
                family   TEXT NOT NULL,
                rx_bytes INTEGER NOT NULL,
                tx_bytes INTEGER NOT NULL,
                PRIMARY KEY (day, app, family)
            );
            CREATE TABLE IF NOT EXISTS traffic_app_hour (
                hour     TEXT NOT NULL,
                app      TEXT NOT NULL,
                family   TEXT NOT NULL,
                rx_bytes INTEGER NOT NULL,
                tx_bytes INTEGER NOT NULL,
                PRIMARY KEY (hour, app, family)
            );
            CREATE INDEX IF NOT EXISTS idx_app_hour_hour ON traffic_app_hour (hour);",
        )
    }

    /// Records the bytes of one packet into the live, in-memory accumulator.
    pub fn record(&self, adapter: &Arc<str>, family: Family, dir: Dir, bytes: u64) {
        let key = BucketKey {
            adapter: Arc::clone(adapter),
            family,
            dir,
            minute: now_minute(),
        };
        let mut live = self.live.lock().expect("live buckets lock poisoned");
        live.entry(key).and_modify(|agg| {
            agg.bytes += bytes;
            agg.pkts += 1;
        }).or_insert(BucketAgg { bytes, pkts: 1 });
    }

    /// 记录一个包的应用流量（按进程）——生产路径已改为按秒批量
    /// `record_app_totals`，此方法仅保留给单元测试使用。
    #[cfg(test)]
    pub fn record_app(&self, app: &str, family: Family, dir: Dir, bytes: u64) {
        let (rx, tx) = if dir == Dir::Rx { (bytes, 0) } else { (0, bytes) };
        self.record_app_totals(app, family, rx, tx);
    }

    /// 记录一段按秒聚合的应用流量（抓包线程每秒批量调用，避免每包分配/哈希开销）。
    /// 按「本地小时桶 × 进程 × IP 族」累计，落盘进 traffic_app_hour，
    /// 支撑任意时间范围的应用明细查询；天级汇总表由 flush 时从该表 rollup。
    pub fn record_app_totals(&self, app: &str, family: Family, rx: u64, tx: u64) {
        if rx == 0 && tx == 0 {
            return;
        }
        let key = AppHourKey {
            hour: local_hour_label(),
            app: app.to_string(),
            family,
        };
        let mut live = self.app_live.lock().expect("app live buckets lock poisoned");
        live.entry(key)
            .and_modify(|agg| {
                agg.rx += rx;
                agg.tx += tx;
            })
            .or_insert(AppAgg { rx, tx });
    }

    /// Drains the live buckets into the database and refreshes rollups and retention.
    pub fn flush(&self) -> Result<(), rusqlite::Error> {
        let drained: HashMap<BucketKey, BucketAgg> = std::mem::take(
            &mut *self.live.lock().expect("live buckets lock poisoned"),
        );

        let mut conn = self.conn.lock().expect("db lock poisoned");
        if !drained.is_empty() {
            let tx = conn.transaction()?;
            let mut stmt = tx.prepare(
                "INSERT INTO traffic_minute (adapter, family, dir, ts, bytes, pkts)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(adapter, family, dir, ts) DO UPDATE
                 SET bytes = bytes + excluded.bytes, pkts = pkts + excluded.pkts",
            )?;
            for (key, agg) in &drained {
                stmt.execute(rusqlite::params![
                    key.adapter.as_ref(),
                    key.family.label(),
                    key.dir.label(),
                    // the bucket key is minute-resolution; the column stores epoch seconds
                    key.minute * 60,
                    i64::try_from(agg.bytes).unwrap_or(i64::MAX),
                    i64::try_from(agg.pkts).unwrap_or(i64::MAX),
                ])?;
            }
            drop(stmt);
            tx.commit()?;
        }

        // rollups use REPLACE semantics: recomputed from source tables, so they are idempotent
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO traffic_day (bucket, adapter, family, dir, bytes, pkts)
             SELECT strftime('%Y-%m-%d', ts, 'unixepoch', 'localtime'), adapter, family, dir,
                    SUM(bytes), SUM(pkts)
             FROM traffic_minute
             WHERE ts >= ?1
             GROUP BY 1, 2, 3, 4
             ON CONFLICT(bucket, adapter, family, dir) DO UPDATE
             SET bytes = excluded.bytes, pkts = excluded.pkts",
            rusqlite::params![now_secs() - 2 * 86400],
        )?;
        tx.execute(
            "INSERT INTO traffic_month (bucket, adapter, family, dir, bytes, pkts)
             SELECT substr(bucket, 1, 7), adapter, family, dir, SUM(bytes), SUM(pkts)
             FROM traffic_day
             WHERE bucket >= strftime('%Y-%m', 'now', 'localtime', '-1 month')
             GROUP BY 1, 2, 3, 4
             ON CONFLICT(bucket, adapter, family, dir) DO UPDATE
             SET bytes = excluded.bytes, pkts = excluded.pkts",
            [],
        )?;
        tx.execute(
            "DELETE FROM traffic_minute WHERE ts < ?1",
            rusqlite::params![now_secs() - RETENTION_MINUTE_DAYS * 86400],
        )?;
        tx.commit()?;

        // 应用每小时流量：增量 upsert 进 traffic_app_hour（无门槛全量明细），
        // 再由该表 rollup 出天级汇总 traffic_app_day（近 2 天 REPLACE 幂等重算）。
        // 天级表只清理「过去日期」中未达门槛的行；小时明细按保留期清理。
        let drained_apps: HashMap<AppHourKey, AppAgg> = std::mem::take(
            &mut *self.app_live.lock().expect("app live buckets lock poisoned"),
        );
        if !drained_apps.is_empty() {
            let tx = conn.transaction()?;
            {
                let mut stmt = tx.prepare(
                    "INSERT INTO traffic_app_hour (hour, app, family, rx_bytes, tx_bytes)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(hour, app, family) DO UPDATE
                     SET rx_bytes = rx_bytes + excluded.rx_bytes,
                         tx_bytes = tx_bytes + excluded.tx_bytes",
                )?;
                for (key, agg) in &drained_apps {
                    stmt.execute(rusqlite::params![
                        key.hour,
                        key.app,
                        key.family.label(),
                        i64::try_from(agg.rx).unwrap_or(i64::MAX),
                        i64::try_from(agg.tx).unwrap_or(i64::MAX),
                    ])?;
                }
            }
            tx.commit()?;
        }

        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO traffic_app_day (day, app, family, rx_bytes, tx_bytes)
             SELECT substr(hour, 1, 10), app, family, SUM(rx_bytes), SUM(tx_bytes)
             FROM traffic_app_hour
             WHERE hour >= ?1
             GROUP BY 1, 2, 3
             ON CONFLICT(day, app, family) DO UPDATE
             SET rx_bytes = excluded.rx_bytes, tx_bytes = excluded.tx_bytes",
            rusqlite::params![local_label_of(
                now_secs() - 2 * 86400,
                "%Y-%m-%d 00:00"
            )],
        )?;
        tx.execute(
            "DELETE FROM traffic_app_day
             WHERE day < ?1
               AND (day, app) IN (
                   SELECT day, app FROM traffic_app_day
                   GROUP BY day, app
                   HAVING SUM(rx_bytes + tx_bytes) < ?2
               )",
            rusqlite::params![
                local_day(),
                i64::try_from(APP_DAY_THRESHOLD_BYTES).unwrap_or(i64::MAX)
            ],
        )?;
        tx.execute(
            "DELETE FROM traffic_app_hour WHERE hour < ?1",
            rusqlite::params![local_label_of(
                now_secs() - RETENTION_APP_HOUR_DAYS * 86400,
                "%Y-%m-%d 00:00"
            )],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Runs an aggregated query, returning pivoted buckets ready for charting.
    pub fn query(
        &self,
        granularity: Granularity,
        adapter: Option<&str>,
    ) -> Result<Vec<HistBucket>, rusqlite::Error> {
        let since = now_secs().saturating_sub(granularity.window_secs());
        let (table, bucket_expr) = match granularity {
            Granularity::Hourly => (
                "traffic_minute",
                "strftime('%Y-%m-%d %H:00', ts, 'unixepoch', 'localtime')",
            ),
            Granularity::Daily => ("traffic_day", "bucket"),
            Granularity::Monthly => ("traffic_month", "bucket"),
        };
        // minute rows store epoch seconds and get labeled here in local time;
        // day/month tables already store local-time bucket labels
        let ts_filter = match granularity {
            Granularity::Hourly => "ts >= ?2",
            Granularity::Daily => "bucket >= strftime('%Y-%m-%d', ?2, 'unixepoch', 'localtime')",
            Granularity::Monthly => "bucket >= ?2",
        };
        let sql = format!(
            "SELECT {bucket_expr} AS bucket,
                COALESCE(SUM(CASE WHEN family = 'v4' AND dir = 'rx' THEN bytes END), 0) AS rx_v4,
                COALESCE(SUM(CASE WHEN family = 'v6' AND dir = 'rx' THEN bytes END), 0) AS rx_v6,
                COALESCE(SUM(CASE WHEN family = 'v4' AND dir = 'tx' THEN bytes END), 0) AS tx_v4,
                COALESCE(SUM(CASE WHEN family = 'v6' AND dir = 'tx' THEN bytes END), 0) AS tx_v6
             FROM {table}
             WHERE {ts_filter} AND (?1 IS NULL OR adapter = ?1)
             GROUP BY bucket
             ORDER BY bucket"
        );
        let adapter_value =
            adapter.map_or(Value::Null, |name| Value::Text(name.to_string()));
        let since_value = match granularity {
            Granularity::Monthly => Value::Text(String::new()),
            _ => Value::Integer(since),
        };

        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(
            rusqlite::params_from_iter([adapter_value, since_value]),
            |row| {
                let read = |idx: usize| -> Result<u64, rusqlite::Error> {
                    Ok(u64::try_from(row.get::<_, i64>(idx)?).unwrap_or_default())
                };
                Ok(HistBucket {
                    label: row.get(0)?,
                    rx_v4: read(1)?,
                    rx_v6: read(2)?,
                    tx_v4: read(3)?,
                    tx_v6: read(4)?,
                })
            },
        )?;
        rows.collect()
    }

    /// Returns all adapters known to the history database (name and optional description).
    #[allow(dead_code)]
    pub fn known_adapters(&self) -> Result<Vec<(String, Option<String>)>, rusqlite::Error> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare("SELECT name, desc FROM adapters ORDER BY name")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    }

    /// 查询应用每日流量历史（按 本地日期 × 进程 汇总 v4/v6 收发）。
    /// 已入库的行都满足入库门槛；尚未落盘的内存桶合并进来后按门槛过滤，
    /// 保证刚产生的重流量应用立即可见。
    pub fn query_app_days(&self) -> Result<Vec<AppDayRow>, rusqlite::Error> {
        // (day, app) -> row；BTreeMap 保持日期有序
        let mut map: std::collections::BTreeMap<(String, String), AppDayRow> =
            std::collections::BTreeMap::new();
        {
            let conn = self.conn.lock().expect("db lock poisoned");
            let mut stmt = conn.prepare(
                "SELECT day, app,
                    COALESCE(SUM(CASE WHEN family = 'v4' THEN rx_bytes END), 0),
                    COALESCE(SUM(CASE WHEN family = 'v4' THEN tx_bytes END), 0),
                    COALESCE(SUM(CASE WHEN family = 'v6' THEN rx_bytes END), 0),
                    COALESCE(SUM(CASE WHEN family = 'v6' THEN tx_bytes END), 0)
                 FROM traffic_app_day
                 GROUP BY day, app",
            )?;
            let read = |v: rusqlite::Result<i64, rusqlite::Error>| -> u64 {
                v.map(|n| u64::try_from(n).unwrap_or_default()).unwrap_or_default()
            };
            let rows = stmt.query_map([], |row| {
                let day: String = row.get(0)?;
                let app: String = row.get(1)?;
                Ok(AppDayRow {
                    rx_v4: read(row.get(2)),
                    tx_v4: read(row.get(3)),
                    rx_v6: read(row.get(4)),
                    tx_v6: read(row.get(5)),
                    day: day.clone(),
                    app,
                })
            })?;
            for r in rows {
                let r = r?;
                map.insert((r.day.clone(), r.app.clone()), r);
            }
        }
        // 合并尚未落盘的内存累计（内存键为小时桶，这里归并到天）
        let live = self.app_live.lock().expect("app live buckets lock poisoned");
        for (key, agg) in live.iter() {
            let day = key.hour.get(..10).unwrap_or("").to_string();
            let e = map
                .entry((day.clone(), key.app.clone()))
                .or_insert_with(|| AppDayRow {
                    day,
                    app: key.app.clone(),
                    rx_v4: 0,
                    tx_v4: 0,
                    rx_v6: 0,
                    tx_v6: 0,
                });
            let (rx, tx) = match key.family {
                Family::V4 => (&mut e.rx_v4, &mut e.tx_v4),
                Family::V6 => (&mut e.rx_v6, &mut e.tx_v6),
            };
            *rx += agg.rx;
            *tx += agg.tx;
        }
        drop(live);
        // 门槛过滤：合并后合计不足 100MB 的应用不展示
        let mut rows: Vec<AppDayRow> = map
            .into_values()
            .filter(|r| r.rx_v4 + r.tx_v4 + r.rx_v6 + r.tx_v6 >= APP_DAY_THRESHOLD_BYTES)
            .collect();
        // 最近日期在前；同一天内按合计流量降序
        rows.sort_by(|a, b| {
            b.day.cmp(&a.day)
                .then((b.rx_v4 + b.tx_v4 + b.rx_v6 + b.tx_v6)
                    .cmp(&(a.rx_v4 + a.tx_v4 + a.rx_v6 + a.tx_v6)))
        });
        Ok(rows)
    }

    /// 任意时间范围的总流量序列。跨度 ≤48h 且起点在分钟保留期内 →
    /// 查 traffic_minute 按小时桶；否则 → 查 traffic_day 按天桶
    /// （day 表永久保留，超出分钟保留期的范围也能查，但粒度降为天）。
    pub fn query_range(
        &self,
        since_ts: i64,
        until_ts: i64,
        adapter: Option<&str>,
    ) -> Result<RangeSeries, rusqlite::Error> {
        const PIVOT: &str = "COALESCE(SUM(CASE WHEN family = 'v4' AND dir = 'rx' THEN bytes END), 0) AS rx_v4, \
             COALESCE(SUM(CASE WHEN family = 'v6' AND dir = 'rx' THEN bytes END), 0) AS rx_v6, \
             COALESCE(SUM(CASE WHEN family = 'v4' AND dir = 'tx' THEN bytes END), 0) AS tx_v4, \
             COALESCE(SUM(CASE WHEN family = 'v6' AND dir = 'tx' THEN bytes END), 0) AS tx_v6";
        let hourly = until_ts.saturating_sub(since_ts) <= HOUR_BUCKET_MAX_SPAN_SECS
            && since_ts >= now_secs() - RETENTION_MINUTE_DAYS * 86400;
        let (granularity, sql) = if hourly {
            (
                "hour",
                format!(
                    "SELECT strftime('%Y-%m-%d %H:00', ts, 'unixepoch', 'localtime') AS bucket, {PIVOT} \
                     FROM traffic_minute \
                     WHERE ts >= ?2 AND ts <= ?3 AND (?1 IS NULL OR adapter = ?1) \
                     GROUP BY bucket ORDER BY bucket"
                ),
            )
        } else {
            (
                "day",
                format!(
                    "SELECT bucket, {PIVOT} \
                     FROM traffic_day \
                     WHERE bucket >= ?2 AND bucket <= ?3 AND (?1 IS NULL OR adapter = ?1) \
                     GROUP BY bucket ORDER BY bucket"
                ),
            )
        };
        let adapter_value =
            adapter.map_or(Value::Null, |name| Value::Text(name.to_string()));
        let since_value = if hourly {
            Value::Integer(since_ts)
        } else {
            Value::Text(local_label_of(since_ts, "%Y-%m-%d"))
        };
        let until_value = if hourly {
            Value::Integer(until_ts)
        } else {
            Value::Text(local_label_of(until_ts, "%Y-%m-%d"))
        };

        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(
            rusqlite::params_from_iter([adapter_value, since_value, until_value]),
            |row| {
                let read = |idx: usize| -> Result<u64, rusqlite::Error> {
                    Ok(u64::try_from(row.get::<_, i64>(idx)?).unwrap_or_default())
                };
                Ok(HistBucket {
                    label: row.get(0)?,
                    rx_v4: read(1)?,
                    rx_v6: read(2)?,
                    tx_v4: read(3)?,
                    tx_v6: read(4)?,
                })
            },
        )?;
        Ok(RangeSeries {
            granularity: granularity.to_string(),
            buckets: rows.collect::<Result<Vec<_>, _>>()?,
        })
    }

    /// 任意时间范围内按应用的流量聚合：traffic_app_hour 全量明细
    /// （无门槛）+ 尚未落盘的内存桶，按合计流量降序。
    pub fn query_app_range(
        &self,
        since_ts: i64,
        until_ts: i64,
    ) -> Result<Vec<AppUsageRow>, rusqlite::Error> {
        let since_label = local_label_of(since_ts, "%Y-%m-%d %H:00");
        let until_label = local_label_of(until_ts, "%Y-%m-%d %H:00");
        let mut map: HashMap<String, AppUsageRow> = HashMap::new();
        {
            let conn = self.conn.lock().expect("db lock poisoned");
            let mut stmt = conn.prepare(
                "SELECT app, \
                    COALESCE(SUM(CASE WHEN family = 'v4' THEN rx_bytes END), 0), \
                    COALESCE(SUM(CASE WHEN family = 'v4' THEN tx_bytes END), 0), \
                    COALESCE(SUM(CASE WHEN family = 'v6' THEN rx_bytes END), 0), \
                    COALESCE(SUM(CASE WHEN family = 'v6' THEN tx_bytes END), 0) \
                 FROM traffic_app_hour \
                 WHERE hour >= ?1 AND hour <= ?2 \
                 GROUP BY app",
            )?;
            let read = |v: rusqlite::Result<i64, rusqlite::Error>| -> u64 {
                v.map(|n| u64::try_from(n).unwrap_or_default()).unwrap_or_default()
            };
            let rows = stmt.query_map(rusqlite::params![since_label, until_label], |row| {
                Ok(AppUsageRow {
                    app: row.get(0)?,
                    rx_v4: read(row.get(1)),
                    tx_v4: read(row.get(2)),
                    rx_v6: read(row.get(3)),
                    tx_v6: read(row.get(4)),
                })
            })?;
            for r in rows {
                let r = r?;
                map.insert(r.app.clone(), r);
            }
        }
        // 合并尚未落盘的内存累计（键为小时桶，逐桶判断是否落在范围内）
        let live = self.app_live.lock().expect("app live buckets lock poisoned");
        for (key, agg) in live.iter() {
            if key.hour.as_str() < since_label.as_str()
                || key.hour.as_str() > until_label.as_str()
            {
                continue;
            }
            let e = map.entry(key.app.clone()).or_insert_with(|| AppUsageRow {
                app: key.app.clone(),
                rx_v4: 0,
                tx_v4: 0,
                rx_v6: 0,
                tx_v6: 0,
            });
            match key.family {
                Family::V4 => {
                    e.rx_v4 += agg.rx;
                    e.tx_v4 += agg.tx;
                }
                Family::V6 => {
                    e.rx_v6 += agg.rx;
                    e.tx_v6 += agg.tx;
                }
            }
        }
        drop(live);
        let mut rows: Vec<AppUsageRow> = map.into_values().collect();
        rows.sort_by(|a, b| {
            (b.rx_v4 + b.tx_v4 + b.rx_v6 + b.tx_v6)
                .cmp(&(a.rx_v4 + a.tx_v4 + a.rx_v6 + a.tx_v6))
        });
        Ok(rows)
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn now_minute() -> i64 {
    now_secs() / 60
}

/// 当前本地日期，`YYYY-MM-DD`（应用每日流量的键）
fn local_day() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// 当前本地小时桶标签，`YYYY-MM-DD HH:00`（应用每小时流量的键）
fn local_hour_label() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:00").to_string()
}

/// 把 UTC epoch 秒格式化为本地时间标签；无效时间戳回退为空串
/// （用于 SQL 文本比较时表示该侧不设限）。
fn local_label_of(ts: i64, fmt: &str) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|d| d.format(fmt).to_string())
        .unwrap_or_default()
}

static STORE: OnceLock<HistoryStore> = OnceLock::new();

/// Initializes the global store at the given path and spawns the background flusher.
/// No-op if the store is already initialized; errors are logged, never fatal.
pub fn init_at(path: &Path) {
    if STORE.get().is_some() {
        return;
    }
    match HistoryStore::open(path) {
        Ok(store) => {
            if STORE.set(store).is_ok() {
                spawn_flusher();
            }
        }
        Err(e) => {
            eprintln!("Sniffnet error: could not open traffic history database: {e}");
        }
    }
}

/// Initializes the global store at its default location, next to the app config file.
pub fn init() {
    let Some(path) = default_db_path() else {
        eprintln!("Sniffnet error: could not resolve traffic history database path");
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    init_at(&path);
}

fn default_db_path() -> Option<std::path::PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(|d| {
            std::path::PathBuf::from(d)
                .join("glassnet")
                .join("traffic_history.db")
        })
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var_os("HOME").map(|d| {
            std::path::PathBuf::from(d)
                .join(".local/share/glassnet/traffic_history.db")
        })
    }
}

fn spawn_flusher() {
    let _ = std::thread::Builder::new()
        .name("thread_traffic_history_flush".to_string())
        .spawn(|| {
            loop {
                std::thread::sleep(std::time::Duration::from_secs(FLUSH_INTERVAL_SECS));
                if let Some(store) = STORE.get() {
                    // soft logging only: the flusher must never take the app down
                    if let Err(e) = store.flush() {
                        eprintln!("Sniffnet error: traffic history flush failed: {e}");
                    }
                }
            }
        });
}

/// Records packet bytes into the global store; silent no-op before `init`.
pub fn record(adapter: &Arc<str>, family: Family, dir: Dir, bytes: u64) {
    if let Some(store) = STORE.get() {
        store.record(adapter, family, dir, bytes);
    }
}

/// Queries the global store; silent no-op (empty result) before `init`.
pub fn query(granularity: Granularity, adapter: Option<&str>) -> Vec<HistBucket> {
    STORE
        .get()
        .and_then(|store| store.query(granularity, adapter).ok())
        .unwrap_or_default()
}

/// 记录按秒聚合的应用流量；silent no-op before `init`。
pub fn record_app_totals(app: &str, family: Family, rx: u64, tx: u64) {
    if let Some(store) = STORE.get() {
        store.record_app_totals(app, family, rx, tx);
    }
}

/// Queries per-app daily traffic history; silent no-op (empty result) before `init`.
pub fn query_app_days() -> Vec<AppDayRow> {
    STORE
        .get()
        .and_then(|store| store.query_app_days().ok())
        .unwrap_or_default()
}

/// 任意时间范围的总流量序列；silent no-op（空序列）before `init`。
pub fn query_range(since_ts: i64, until_ts: i64, adapter: Option<&str>) -> Option<RangeSeries> {
    STORE
        .get()
        .and_then(|store| store.query_range(since_ts, until_ts, adapter).ok())
}

/// 任意时间范围的应用流量聚合；silent no-op（空结果）before `init`。
pub fn query_app_range(since_ts: i64, until_ts: i64) -> Vec<AppUsageRow> {
    STORE
        .get()
        .and_then(|store| store.query_app_range(since_ts, until_ts).ok())
        .unwrap_or_default()
}

/// 立即落盘（应用退出时调用，避免丢失最近一个 flush 周期的数据）。
pub fn flush_now() {
    if let Some(store) = STORE.get() {
        if let Err(e) = store.flush() {
            eprintln!("Sniffnet error: traffic history flush on exit failed: {e}");
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> HistoryStore {
        HistoryStore::open(Path::new(":memory:")).expect("in-memory store")
    }

    fn arc(s: &str) -> Arc<str> {
        Arc::from(s)
    }

    #[test]
    fn test_record_and_flush_accumulates() {
        let store = store();
        let adapter = arc("eth0");
        store.record(&adapter, Family::V4, Dir::Rx, 100);
        store.record(&adapter, Family::V4, Dir::Rx, 50);
        store.record(&adapter, Family::V6, Dir::Tx, 10);
        store.flush().expect("flush");

        // simulate a second flush interval for the same minute: increments must add up
        store.record(&adapter, Family::V4, Dir::Rx, 25);
        store.flush().expect("flush 2");

        let buckets = store
            .query(Granularity::Hourly, None)
            .expect("query");
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[0].rx_v4, 175);
        assert_eq!(buckets[0].rx_v6, 0);
        assert_eq!(buckets[0].tx_v6, 10);
        assert_eq!(buckets[0].tx_v4, 0);
    }

    #[test]
    fn test_query_filters_by_adapter() {
        let store = store();
        let a = arc("eth0");
        let w = arc("wlan0");
        store.record(&a, Family::V4, Dir::Rx, 100);
        store.record(&w, Family::V4, Dir::Rx, 200);
        store.flush().expect("flush");

        assert_eq!(store.query(Granularity::Hourly, Some("eth0")).expect("q")[0].total(), 100);
        assert_eq!(store.query(Granularity::Hourly, Some("wlan0")).expect("q")[0].total(), 200);
        assert_eq!(store.query(Granularity::Hourly, None).expect("q")[0].total(), 300);
    }

    #[test]
    fn test_daily_and_monthly_rollups() {
        let store = store();
        let adapter = arc("eth0");
        store.record(&adapter, Family::V4, Dir::Rx, 100);
        store.record(&adapter, Family::V6, Dir::Tx, 40);
        store.flush().expect("flush");

        // minute rows get pruned 90 days later, but day/month rollups must remain
        store
            .conn
            .lock()
            .expect("db")
            .execute("DELETE FROM traffic_minute", [])
            .expect("prune");

        let daily = store.query(Granularity::Daily, None).expect("daily");
        assert_eq!(daily.len(), 1);
        assert_eq!(daily[0].rx_v4, 100);
        assert_eq!(daily[0].tx_v6, 40);
        assert_eq!(daily[0].label.len(), 10); // YYYY-MM-DD

        let monthly = store.query(Granularity::Monthly, None).expect("monthly");
        assert_eq!(monthly.len(), 1);
        assert_eq!(monthly[0].total(), 140);
        assert_eq!(monthly[0].label.len(), 7); // YYYY-MM
    }

    #[test]
    fn test_day_rollup_is_replace_not_accumulate() {
        let store = store();
        let adapter = arc("eth0");
        store.record(&adapter, Family::V4, Dir::Rx, 100);
        store.flush().expect("flush 1");
        store.record(&adapter, Family::V4, Dir::Rx, 100);
        store.flush().expect("flush 2");

        let daily = store.query(Granularity::Daily, None).expect("daily");
        assert_eq!(daily[0].rx_v4, 200);
    }

    #[test]
    fn test_app_daily_accumulates_across_flushes_and_filters_threshold() {
        let store = store();
        // 第一段 60MB：未达 100MB 门槛，查询不可见（行保留在库中等待次日判定）
        store.record_app("heavy.exe", Family::V4, Dir::Rx, 60_000_000);
        store.flush().expect("flush 1");
        assert!(store.query_app_days().expect("q").is_empty());

        // 第二段 60MB：增量加总 120MB 超过门槛，v4/v6 明细正确
        store.record_app("heavy.exe", Family::V4, Dir::Rx, 60_000_000);
        store.record_app("heavy.exe", Family::V6, Dir::Tx, 50);
        store.flush().expect("flush 2");
        let rows = store.query_app_days().expect("q 2");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].app, "heavy.exe");
        assert_eq!(rows[0].rx_v4, 120_000_000);
        assert_eq!(rows[0].tx_v6, 50);

        // 低流量应用（99.99MB，差 0.01MB 达标）当天同样不展示
        store.record_app("small.exe", Family::V4, Dir::Tx, 99_999_999);
        store.flush().expect("flush 3");
        let rows = store.query_app_days().expect("q 3");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].app, "heavy.exe");
    }

    #[test]
    fn test_app_daily_prunes_stale_subthreshold_rows() {
        let store = store();
        // 注入历史残留：昨日低于门槛的行应被清理，达标的保留
        store
            .conn
            .lock()
            .expect("db")
            .execute_batch(
                "INSERT INTO traffic_app_day VALUES ('2000-01-01','old.exe','v4',100,0);
                 INSERT INTO traffic_app_day VALUES ('2000-01-01','big.exe','v4',2000000000,0);",
            )
            .expect("seed");
        store.record_app("heavy.exe", Family::V4, Dir::Rx, 1_500_000_000);
        store.flush().expect("flush");

        let rows = store.query_app_days().expect("q");
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|r| r.app == "big.exe" && r.day == "2000-01-01"));
        assert!(rows.iter().any(|r| r.app == "heavy.exe" && r.rx_v4 == 1_500_000_000));
        assert!(!rows.iter().any(|r| r.app == "old.exe"));
    }

    #[test]
    fn test_app_hour_range_query() {
        let store = store();
        // 当前小时记录并 flush，范围查询按应用聚合 v4/v6
        store.record_app("a.exe", Family::V4, Dir::Rx, 100);
        store.record_app("a.exe", Family::V6, Dir::Tx, 30);
        store.record_app("b.exe", Family::V4, Dir::Rx, 50);
        store.flush().expect("flush");

        let now = now_secs();
        let rows = store.query_app_range(now - 3600, now + 3600).expect("range");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].app, "a.exe"); // 130 > 80，降序
        assert_eq!(rows[0].rx_v4, 100);
        assert_eq!(rows[0].tx_v6, 30);
        assert_eq!(rows[1].app, "b.exe");
        assert_eq!(rows[1].rx_v4, 50);

        // 完全范围外的查询为空
        let empty = store
            .query_app_range(now - 20 * 86400, now - 10 * 86400)
            .expect("old");
        assert!(empty.is_empty());

        // 未落盘的内存桶同样并入范围内
        store.record_app("c.exe", Family::V4, Dir::Rx, 7);
        let rows = store.query_app_range(now - 300, now + 300).expect("live");
        let c = rows.iter().find(|r| r.app == "c.exe").expect("c.exe live");
        assert_eq!(c.rx_v4, 7);
    }

    #[test]
    fn test_app_hour_day_rollup_and_threshold() {
        let store = store();
        // 小时明细全量落库；天级汇总表仍按 100MB 门槛过滤
        store.record_app("big.exe", Family::V4, Dir::Rx, 150_000_000);
        store.record_app("small.exe", Family::V4, Dir::Rx, 50_000_000);
        store.flush().expect("flush");

        let days = store.query_app_days().expect("days");
        assert_eq!(days.len(), 1);
        assert_eq!(days[0].app, "big.exe");
        assert_eq!(days[0].rx_v4, 150_000_000);

        // 但小时明细范围查询不设门槛，小应用也可见
        let now = now_secs();
        let rows = store.query_app_range(now - 3600, now + 3600).expect("range");
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn test_query_range_hourly_and_daily() {
        let store = store();
        let adapter = arc("eth0");
        store.record(&adapter, Family::V4, Dir::Rx, 300);
        store.flush().expect("flush");
        let now = now_secs();

        // 12 小时范围（≤48h 且在保留期内）→ 小时桶
        let series = store.query_range(now - 12 * 3600, now, None).expect("hourly");
        assert_eq!(series.granularity, "hour");
        assert_eq!(series.buckets.len(), 1);
        assert_eq!(series.buckets[0].rx_v4, 300);

        // 10 天范围 → 天桶（来自 traffic_day rollup）
        let series = store
            .query_range(now - 10 * 86400, now, Some("eth0"))
            .expect("daily");
        assert_eq!(series.granularity, "day");
        assert_eq!(series.buckets.len(), 1);
        assert_eq!(series.buckets[0].rx_v4, 300);

        // 网卡过滤生效
        let series = store
            .query_range(now - 12 * 3600, now, Some("nope"))
            .expect("filtered");
        assert!(series.buckets.is_empty());
    }

}
