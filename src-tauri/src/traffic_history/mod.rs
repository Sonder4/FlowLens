//! Persistent traffic history subsystem (SQLite): per adapter, IPv4/IPv6,
//! direction, aggregated in minute buckets with permanent day/month rollups.

pub mod store;
pub mod types;

pub use store::{flush_now, init, query, query_app_days, record, record_app};
pub use types::{AppDayRow, Dir, Family, Granularity, HistBucket};
