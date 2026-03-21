use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub redis: RedisConfig,
    pub telemetry: TelemetryConfig,
    pub auth: AuthConfig,
    pub routing: RoutingConfig,
    pub circuit_breaker: CircuitBreakerConfig,
    pub guardrails: GuardrailsConfig,
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

#[derive(Debug, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
    #[serde(default = "default_max_connections")]
    pub max_connections: u32,
}

#[derive(Debug, Deserialize)]
pub struct RedisConfig {
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub struct TelemetryConfig {
    pub otlp_endpoint: String,
    #[serde(default = "default_service_name")]
    pub service_name: String,
}

#[derive(Debug, Deserialize)]
pub struct AuthConfig {
    #[serde(default = "default_key_prefix")]
    pub key_prefix: String,
    #[serde(default = "default_hash_algorithm")]
    pub hash_algorithm: String,
}

#[derive(Debug, Deserialize)]
pub struct RoutingConfig {
    #[serde(default = "default_strategy")]
    pub default_strategy: RoutingStrategy,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RoutingStrategy {
    RoundRobin,
    Weighted,
    Latency,
    HealthAware,
}

#[derive(Debug, Deserialize)]
pub struct CircuitBreakerConfig {
    #[serde(default = "default_failure_threshold")]
    pub failure_threshold: u32,
    #[serde(default = "default_cooldown_seconds")]
    pub cooldown_seconds: u64,
    #[serde(default = "default_half_open_max_requests")]
    pub half_open_max_requests: u32,
}

#[derive(Debug, Deserialize)]
pub struct GuardrailsConfig {
    #[serde(default = "default_true")]
    pub enable_injection_filter: bool,
    #[serde(default = "default_true")]
    pub enable_secret_scanner: bool,
    #[serde(default = "default_max_request_size")]
    pub max_request_size_bytes: usize,
}

#[derive(Debug, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    #[serde(rename = "type")]
    pub provider_type: String,
    pub base_url: String,
    #[serde(default)]
    pub api_key: Option<String>,
    pub models: Vec<String>,
    #[serde(default)]
    pub cost_per_input_token: Option<f64>,
    #[serde(default)]
    pub cost_per_output_token: Option<f64>,
    #[serde(default)]
    pub priority: i32,
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

impl Config {
    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let resolved = resolve_env_vars(&content);
        let config: Config = toml::from_str(&resolved)?;
        Ok(config)
    }
}

/// `${VAR_NAME}` → значение из env, паникует если переменная не задана.
/// `${VAR_NAME:-default}` → значение из env, или default если не задана.
fn resolve_env_vars(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '$' && chars.peek() == Some(&'{') {
            chars.next(); // skip '{'
            let mut var_expr = String::new();
            for ch in chars.by_ref() {
                if ch == '}' {
                    break;
                }
                var_expr.push(ch);
            }
            let value = if let Some((name, default)) = var_expr.split_once(":-") {
                std::env::var(name).unwrap_or_else(|_| default.to_string())
            } else {
                std::env::var(&var_expr)
                    .unwrap_or_else(|_| panic!("env var {var_expr} is required but not set"))
            };
            result.push_str(&value);
        } else {
            result.push(ch);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_resolve_env_vars() {
        // SAFETY: тесты запускаются до создания доп. потоков в этом процессе
        unsafe { std::env::set_var("TEST_GW_VAR", "hello") };
        assert_eq!(resolve_env_vars("${TEST_GW_VAR}"), "hello");
        assert_eq!(resolve_env_vars("${MISSING_VAR:-fallback}"), "fallback");
        assert_eq!(resolve_env_vars("no vars here"), "no vars here");
    }

    #[test]
    fn test_load_minimal_config() {
        let toml_content = r#"
[server]
host = "127.0.0.1"
port = 3000

[database]
url = "postgres://localhost/test"

[redis]
url = "redis://localhost"

[telemetry]
otlp_endpoint = "http://localhost:4317"

[auth]

[routing]

[circuit_breaker]

[guardrails]

[[providers]]
name = "mock"
type = "mock"
base_url = "http://localhost:9001"
models = ["mock-fast"]
"#;
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(toml_content.as_bytes()).unwrap();

        let config = Config::load(tmp.path()).unwrap();
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.port, 3000);
        assert_eq!(config.providers.len(), 1);
        assert_eq!(config.routing.default_strategy, RoutingStrategy::RoundRobin);
    }
}
