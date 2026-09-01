//! Persistent traffic history subsystem (SQLite): per adapter, IPv4/IPv6,
//! direction, aggregated in minute buckets with permanent day/month rollups.
//! Per-app traffic is recorded at hour granularity (traffic_app_hour,
//! full detail within the retention window) with a permanent day-level
//! summary table (traffic_app_day, threshold-filtered).

pub mod store;
pub mod types;

pub use store::{
    flush_now, init, query, query_app_days, query_app_range, query_range, record,
    record_app_totals,
};
pub use types::{AppDayRow, Dir, Family, Granularity, HistBucket, RangeSeries};
