use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RouteSettings {
    pub probe_amount: u64,
}

impl Default for RouteSettings {
    fn default() -> Self {
        Self {
            probe_amount: 1_000_000,
        }
    }
}
