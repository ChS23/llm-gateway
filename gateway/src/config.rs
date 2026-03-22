use std::fmt;
use std::path::Path;

use serde::Deserialize;

// -- Error type ---------------------------------------------------------------

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Parse(toml::de::Error),
    EnvVar(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "config I/O error: {e}"),
            Self::Parse(e) => write!(f, "config parse error: {e}"),
            Self::EnvVar(msg) => write!(f, "config env error: {msg}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<std::io::Error> for ConfigError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<toml::de::Error> for ConfigError {
    fn from(e: toml::de::Error) -> Self {
        Self::Parse(e)
    }
}

// -- Config structs -----------------------------------------------------------

#[derive(Deserialize)]
#[allow(dead_code)] // Fields used in Phase 2/3
pub struct Config {
    pub server: ServerConfig,
    #[serde(default)]
    pub database: DatabaseConfig,
    #[serde(default)]
    pub redis: RedisConfig,
    pub telemetry: TelemetryConfig,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub routing: RoutingConfig,
    #[serde(default)]
    pub circuit_breaker: CircuitBreakerConfig,
    #[serde(default)]
    pub guardrails: GuardrailsConfig,
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("server", &self.server)
            .field("routing", &self.routing)
            .field(
                "providers",
                &format_args!("[{} providers]", self.providers.len()),
            )
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

#[derive(Debug, Deserialize, Default)]
#[allow(dead_code)]
pub struct DatabaseConfig {
    #[serde(default)]
    pub url: String,
    #[serde(default = "default_max_connections")]
    pub max_connections: u32,
}

#[derive(Debug, Deserialize, Default)]
#[allow(dead_code)]
pub struct RedisConfig {
    #[serde(default)]
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub struct TelemetryConfig {
    pub otlp_endpoint: String,
    #[serde(default = "default_service_name")]
    pub service_name: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct AuthConfig {
    #[serde(default = "default_key_prefix")]
    pub key_prefix: String,
    #[serde(default = "default_hash_algorithm")]
    pub hash_algorithm: String,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            key_prefix: default_key_prefix(),
            hash_algorithm: default_hash_algorithm(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct RoutingConfig {
    #[serde(default = "default_strategy")]
    pub default_strategy: RoutingStrategy,
    /// SSE failover: cancel and retry on another provider if first token
    /// doesn't arrive within this timeout. 0 = disabled.
    #[serde(default = "default_ttft_timeout_ms")]
    pub ttft_timeout_ms: u64,
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            default_strategy: default_strategy(),
            ttft_timeout_ms: default_ttft_timeout_ms(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RoutingStrategy {
    RoundRobin,
    Weighted,
    Latency,
    LeastConnections,
    HealthAware,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CircuitBreakerConfig {
    #[serde(default = "default_failure_threshold")]
    pub failure_threshold: u32,
    #[serde(default = "default_cooldown_seconds")]
    pub cooldown_seconds: u64,
    #[serde(default = "default_half_open_max_requests")]
    pub half_open_max_requests: u32,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: default_failure_threshold(),
            cooldown_seconds: default_cooldown_seconds(),
            half_open_max_requests: default_half_open_max_requests(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct GuardrailsConfig {
    #[serde(default = "default_true")]
    pub enable_injection_filter: bool,
    #[serde(default = "default_true")]
    pub enable_secret_scanner: bool,
    #[serde(default = "default_max_request_size")]
    pub max_request_size_bytes: usize,
}

impl Default for GuardrailsConfig {
    fn default() -> Self {
        Self {
            enable_injection_filter: true,
            enable_secret_scanner: true,
            max_request_size_bytes: default_max_request_size(),
        }
    }
}

#[derive(Deserialize)]
#[allow(dead_code)]
pub struct ProviderConfig {
    pub name: String,
    #[serde(rename = "type")]
    pub provider_type: String,
    pub base_url: String,
    #[serde(default, deserialize_with = "deserialize_non_empty")]
    pub api_key: Option<String>,
    pub models: Vec<String>,
    #[serde(default)]
    pub cost_per_input_token: Option<f64>,
    #[serde(default)]
    pub cost_per_output_token: Option<f64>,
    #[serde(default)]
    pub priority: i32,
    #[serde(default = "default_weight")]
    pub weight: u32,
}

impl fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("name", &self.name)
            .field("type", &self.provider_type)
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("models", &self.models)
            .field("weight", &self.weight)
            .finish_non_exhaustive()
    }
}

/// Deserializes a string into Option: empty string becomes None.
fn deserialize_non_empty<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt: Option<String> = Option::deserialize(deserializer)?;
    Ok(opt.filter(|s| !s.is_empty()))
}

fn default_host() -> String {
    "0.0.0.0".into()
}
fn default_port() -> u16 {
    8080
}
fn default_max_connections() -> u32 {
    20
}
fn default_service_name() -> String {
    "llm-gateway".into()
}
fn default_key_prefix() -> String {
    "sk-gw".into()
}
fn default_hash_algorithm() -> String {
    "sha256".into()
}
fn default_strategy() -> RoutingStrategy {
    RoutingStrategy::RoundRobin
}
fn default_failure_threshold() -> u32 {
    5
}
fn default_cooldown_seconds() -> u64 {
    30
}
fn default_half_open_max_requests() -> u32 {
    3
}
fn default_true() -> bool {
    true
}
fn default_max_request_size() -> usize {
    1_048_576
}
fn default_weight() -> u32 {
    1
}
fn default_ttft_timeout_ms() -> u64 {
    5000
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path)?;
        let resolved = resolve_env_vars(&content)?;
        let config: Config = toml::from_str(&resolved)?;
        Ok(config)
    }
}

/// Resolves `${VAR}` (required) and `${VAR:-default}` (with fallback) in input.
fn resolve_env_vars(input: &str) -> Result<String, ConfigError> {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '$' && chars.peek() == Some(&'{') {
            chars.next();
            let mut var_expr = String::new();
            let mut found_close = false;
            for ch in chars.by_ref() {
                if ch == '}' {
                    found_close = true;
                    break;
                }
                var_expr.push(ch);
            }
            if !found_close {
                return Err(ConfigError::EnvVar(format!(
                    "unclosed expression: ${{{var_expr}"
                )));
            }
            let value = if let Some((name, default)) = var_expr.split_once(":-") {
                std::env::var(name).unwrap_or_else(|_| default.to_string())
            } else {
                std::env::var(&var_expr).map_err(|_| {
                    ConfigError::EnvVar(format!("required env var '{var_expr}' is not set"))
                })?
            };
            result.push_str(&value);
        } else {
            result.push(ch);
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_resolve_env_vars() {
        // SAFETY: tests run before additional threads are spawned
        unsafe { std::env::set_var("TEST_GW_VAR", "hello") };
        assert_eq!(resolve_env_vars("${TEST_GW_VAR}").unwrap(), "hello");
        assert_eq!(
            resolve_env_vars("${MISSING_VAR:-fallback}").unwrap(),
            "fallback"
        );
        assert_eq!(resolve_env_vars("no vars here").unwrap(), "no vars here");
    }

    #[test]
    fn test_resolve_missing_required_var() {
        let result = resolve_env_vars("${DEFINITELY_MISSING_VAR_XYZ}");
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_unclosed_expression() {
        let result = resolve_env_vars("${UNCLOSED");
        assert!(result.is_err());
    }

    #[test]
    fn test_load_minimal_config() {
        let toml_content = r#"
[server]
[telemetry]
otlp_endpoint = "http://localhost:4317"
"#;
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(toml_content.as_bytes()).unwrap();

        let config = Config::load(tmp.path()).unwrap();
        assert_eq!(config.server.port, 8080);
        assert_eq!(config.routing.default_strategy, RoutingStrategy::RoundRobin);
        assert!(config.providers.is_empty());
    }
}
