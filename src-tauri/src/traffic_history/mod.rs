//! Persistent traffic history subsystem (SQLite): per adapter, IPv4/IPv6,
//! direction, aggregated in minute buckets with permanent day/month rollups.

pub mod store;
pub mod types;

pub use store::{init, query, record};
pub use types::{Dir, Family, Granularity, HistBucket};
