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

use super::types::{BucketAgg, BucketKey, Dir, Family, Granularity, HistBucket};

/// Minute-level details are kept for this many days; day/month rollups are kept forever.
const RETENTION_MINUTE_DAYS: i64 = 90;

/// Flush cadence of the background flusher thread, in seconds.
pub const FLUSH_INTERVAL_SECS: u64 = 60;

pub struct HistoryStore {
    conn: Mutex<Connection>,
    live: Mutex<HashMap<BucketKey, BucketAgg>>,
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
            );",
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
        tx.commit()
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
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn now_minute() -> i64 {
    now_secs() / 60
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

}
