mod builtins;
mod env;
mod eval;
mod expand;
mod value;

use crate::config::ComptimeHttpConfig;

pub use expand::expand_program;

#[derive(Debug, Clone, Default)]
pub struct ComptimeOptions {
    pub http: ComptimeHttpOptions,
}

#[derive(Debug, Clone)]
pub struct ComptimeHttpOptions {
    pub enabled: bool,
    pub allow: Vec<String>,
    pub timeout_ms: u64,
}

impl Default for ComptimeHttpOptions {
    fn default() -> Self {
        Self {
            enabled: false,
            allow: Vec::new(),
            timeout_ms: 5_000,
        }
    }
}

impl From<&ComptimeHttpConfig> for ComptimeHttpOptions {
    fn from(value: &ComptimeHttpConfig) -> Self {
        Self {
            enabled: value.enabled,
            allow: value.allow.clone(),
            timeout_ms: value.timeout_ms,
        }
    }
}
