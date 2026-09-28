use std::path::Path;

use anyhow::Context;
use grpc::GrpcSettings;
use market::{SyncSettings, UniverseConfig};
use route::RouteSettings;
use rpc::RpcSettings;
use serde::Deserialize;
use server::ServerSettings;

const RPC_URL_ENV: &str = "TB_RPC_URL";
const GRPC_URL_ENV: &str = "TB_GRPC_URL";
const GRPC_X_TOKEN_ENV: &str = "TB_GRPC_X_TOKEN";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    #[serde(default = "default_stats_interval_secs")]
    pub stats_interval_secs: u64,
    #[serde(default)]
    pub rpc: RpcSettings,
    #[serde(default)]
    pub grpc: GrpcSettings,
    #[serde(default)]
    pub sync: SyncSettings,
    #[serde(default)]
    pub route: RouteSettings,
    #[serde(default)]
    pub threads: ThreadSettings,
    #[serde(default)]
    pub server: ServerSettings,
    pub universe: UniverseConfig,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThreadSettings {
    pub pipeline: u16,
    pub search: u16,
}

const fn default_stats_interval_secs() -> u64 {
    10
}

pub fn load(path: &Path) -> anyhow::Result<AppConfig> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading config {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing config {}", path.display()))
}

pub struct Secrets {
    pub rpc_url: String,
    pub grpc_url: String,
    pub grpc_x_token: Option<String>,
}

impl Secrets {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Self {
            rpc_url: required(RPC_URL_ENV)?,
            grpc_url: required(GRPC_URL_ENV)?,
            grpc_x_token: std::env::var(GRPC_X_TOKEN_ENV)
                .ok()
                .filter(|t| !t.is_empty()),
        })
    }
}

fn required(name: &str) -> anyhow::Result<String> {
    std::env::var(name)
        .ok()
        .filter(|v| !v.is_empty())
        .with_context(|| format!("environment variable {name} is not set"))
}
