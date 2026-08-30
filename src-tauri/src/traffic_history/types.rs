//! Types shared by the traffic history subsystem:
//! live buckets accumulated in memory and aggregated results returned by queries.



/// IP protocol version of a captured packet.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Family {
    V4,
    V6,
}

impl Family {
    pub fn label(self) -> &'static str {
        match self {
            Self::V4 => "v4",
            Self::V6 => "v6",
        }
    }

    #[cfg(test)]
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "v4" => Some(Self::V4),
            "v6" => Some(Self::V6),
            _ => None,
        }
    }
}

/// Traffic direction, aligned with `TrafficDirection` but named after what the interface does.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Dir {
    Rx,
    Tx,
}

impl Dir {
    pub fn label(self) -> &'static str {
        match self {
            Self::Rx => "rx",
            Self::Tx => "tx",
        }
    }

    #[cfg(test)]
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "rx" => Some(Self::Rx),
            "tx" => Some(Self::Tx),
            _ => None,
        }
    }
}

/// Key of an in-memory accumulation bucket; `minute` is the UTC epoch floored to the minute.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct BucketKey {
    pub adapter: std::sync::Arc<str>,
    pub family: Family,
    pub dir: Dir,
    pub minute: i64,
}

/// Accumulated counters of a single bucket.
#[derive(Clone, Copy, Debug, Default)]
pub struct BucketAgg {
    pub bytes: u64,
    pub pkts: u64,
}

/// Time granularity of a history query.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Granularity {
    /// Last 24 hours, one bucket per hour.
    Hourly,
    /// Last 30 days, one bucket per day.
    Daily,
    /// All recorded months, one bucket per month.
    Monthly,
}

impl Granularity {
    /// Seconds looked back from now.
    pub fn window_secs(self) -> i64 {
        match self {
            Self::Hourly => 24 * 3600,
            Self::Daily => 30 * 86400,
            Self::Monthly => i64::MAX,
        }
    }
}

/// One pivoted data point of a history query, ready for charting.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistBucket {
    /// Human-readable bucket start in local time:
    /// `YYYY-MM-DD HH:00` (hourly), `YYYY-MM-DD` (daily), `YYYY-MM` (monthly).
    pub label: String,
    pub rx_v4: u64,
    pub rx_v6: u64,
    pub tx_v4: u64,
    pub tx_v6: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use etherparse::{Ipv4Extensions, Ipv4Header, NetHeaders};

    #[test]
    fn test_family_from_net_headers() {
        use etherparse::{Ipv4Extensions, Ipv4Header};
        let v4 = Some(NetHeaders::Ipv4(Ipv4Header::default(), Ipv4Extensions::default()));
        assert_eq!(Family::from_net_headers(&v4), Some(Family::V4));
        assert_eq!(Family::from_net_headers(&None), None);
    }

    #[test]
    fn test_labels_roundtrip() {
        for family in [Family::V4, Family::V6] {
            assert_eq!(Family::from_label(family.label()), Some(family));
        }
        for dir in [Dir::Rx, Dir::Tx] {
            assert_eq!(Dir::from_label(dir.label()), Some(dir));
        }
    }
}
