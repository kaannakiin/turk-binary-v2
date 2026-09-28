use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use serde::Deserialize;

use crate::error::ServerError;

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerSettings {
    pub api_addr: SocketAddr,
    pub ops_addr: SocketAddr,
    pub drain_delay_ms: u64,
    pub read_timeout_ms: u64,
    pub shutdown_timeout_ms: u64,
    pub ready: ReadySettings,
    pub quote: QuoteSettings,
}

impl ServerSettings {
    #[must_use]
    pub fn drain_delay(&self) -> Duration {
        Duration::from_millis(self.drain_delay_ms)
    }

    #[must_use]
    pub fn read_timeout(&self) -> Duration {
        Duration::from_millis(self.read_timeout_ms)
    }

    #[must_use]
    pub fn shutdown_timeout(&self) -> Duration {
        Duration::from_millis(self.shutdown_timeout_ms)
    }
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            api_addr: SocketAddr::from((Ipv4Addr::LOCALHOST, 8080)),
            ops_addr: SocketAddr::from((Ipv4Addr::LOCALHOST, 9100)),
            drain_delay_ms: 0,
            read_timeout_ms: 5_000,
            shutdown_timeout_ms: 5_000,
            ready: ReadySettings::default(),
            quote: QuoteSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ReadySettings {
    pub startup_percent: u8,
    pub floor_percent: u8,
    pub max_clock_stall_ms: u64,
}

impl ReadySettings {
    #[must_use]
    pub fn max_clock_stall(&self) -> Duration {
        Duration::from_millis(self.max_clock_stall_ms)
    }

    pub(crate) fn validate(&self) -> Result<(), ServerError> {
        if self.startup_percent > 100 || self.floor_percent > 100 {
            return Err(ServerError::Settings("ready percents are at most 100"));
        }
        Ok(())
    }
}

impl Default for ReadySettings {
    fn default() -> Self {
        Self {
            startup_percent: 90,
            floor_percent: 50,
            max_clock_stall_ms: 10_000,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct QuoteSettings {
    pub default_max_hops: u8,
    pub max_hops: u8,
    pub max_quotes: u32,
    /// `0` quotes every pool of every pair.
    pub per_pair: u8,
    pub max_arrays: u8,
    pub timeout_ms: u64,
    pub max_queued: usize,
}

impl QuoteSettings {
    #[must_use]
    pub fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_ms)
    }

    pub(crate) fn validate(&self) -> Result<(), ServerError> {
        if self.default_max_hops == 0 || self.default_max_hops > self.max_hops {
            return Err(ServerError::Settings(
                "quote.default_max_hops must be between 1 and quote.max_hops",
            ));
        }
        Ok(())
    }
}

impl Default for QuoteSettings {
    fn default() -> Self {
        Self {
            default_max_hops: 3,
            max_hops: 4,
            max_quotes: 100_000,
            per_pair: 2,
            max_arrays: 8,
            timeout_ms: 2_000,
            max_queued: 32,
        }
    }
}

/// `0` takes a quarter of the cores, at least one.
#[must_use]
pub fn search_threads(requested: u16, cores: usize) -> usize {
    match requested {
        0 => (cores / 4).max(1),
        n => usize::from(n),
    }
}
