//! Application configuration management.
//!
//! Provides typed configuration loaded from environment variables with validation.

use std::time::Duration;

/// Application configuration loaded from environment variables.
#[derive(Debug, Clone)]
pub struct Config {
    /// PostgreSQL database connection URL
    pub database_url: String,

    /// Public host URL for webhook callbacks
    pub host_url: String,

    /// Server port to bind to
    pub port: u16,

    /// Database connection pool settings
    pub pool: PoolConfig,

    /// Pagination settings
    pub pagination: PaginationConfig,

    /// Worker settings
    pub worker: WorkerConfig,

    /// Circuit breaker settings for connection pool resilience
    pub circuit_breaker: CircuitBreakerConfig,

    /// Observability settings
    pub observability: ObservabilityConfig,

    /// Security settings
    pub security: SecurityConfig,
}

/// Database connection pool configuration.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Maximum number of connections in the pool
    pub max_size: u32,

    /// Minimum number of idle connections to maintain
    pub min_idle: u32,

    /// Maximum lifetime of a connection
    pub max_lifetime: Duration,

    /// Idle timeout for connections
    pub idle_timeout: Duration,

    /// Connection acquisition timeout
    pub connection_timeout: Duration,

    /// Number of retries when acquiring a connection
    pub acquire_retries: u32,

    /// Delay between retry attempts
    pub retry_delay: Duration,
}

/// Pagination configuration.
#[derive(Debug, Clone)]
pub struct PaginationConfig {
    /// Default number of items per page
    pub default_per_page: i64,

    /// Maximum allowed items per page
    pub max_per_page: i64,
}

/// Worker loop configuration.
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// Interval between worker loop iterations
    pub loop_interval: Duration,

    /// Interval for timeout check loop
    pub timeout_check_interval: Duration,

    /// Interval for batch updater flush
    pub batch_flush_interval: Duration,

    /// Batch update channel capacity
    pub batch_channel_capacity: usize,
}

/// Circuit breaker configuration for connection pool resilience.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Whether the circuit breaker is enabled
    pub enabled: bool,

    /// Number of failures before circuit opens
    pub failure_threshold: u32,

    /// Time window for counting failures (in seconds)
    pub failure_window_secs: u64,

    /// How long to wait before trying half-open (in seconds)
    pub recovery_timeout_secs: u64,

    /// Number of successes in half-open state to close circuit
    pub success_threshold: u32,
}

/// Observability configuration for logging and metrics.
#[derive(Debug, Clone)]
pub struct ObservabilityConfig {
    /// Threshold in milliseconds for slow query warnings
    pub slow_query_threshold_ms: u64,

    /// Whether distributed tracing is enabled
    pub tracing_enabled: bool,

    /// OTLP endpoint for trace export
    pub otlp_endpoint: Option<String>,

    /// Service name for traces
    pub service_name: String,

    /// Sampling ratio (0.0 to 1.0)
    pub sampling_ratio: f64,
}

/// Security configuration for SSRF protection and validation.
#[derive(Debug, Clone)]
pub struct SecurityConfig {
    /// Skip SSRF validation (useful for development/testing)
    /// In debug builds, this defaults to true
    pub skip_ssrf_validation: bool,

    /// List of blocked hostnames for webhook URLs
    pub blocked_hostnames: Vec<String>,

    /// List of blocked hostname suffixes (e.g., ".local", ".internal")
    pub blocked_hostname_suffixes: Vec<String>,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_size: 10,
            min_idle: 5,
            max_lifetime: Duration::from_secs(60 * 60 * 24), // 24 hours
            idle_timeout: Duration::from_secs(60 * 2),       // 2 minutes
            connection_timeout: Duration::from_secs(30),
            acquire_retries: 3,
            retry_delay: Duration::from_millis(100),
        }
    }
}

impl Default for PaginationConfig {
    fn default() -> Self {
        Self {
            default_per_page: 50,
            max_per_page: 100,
        }
    }
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            loop_interval: Duration::from_secs(1),
            timeout_check_interval: Duration::from_secs(1),
            batch_flush_interval: Duration::from_millis(100),
            batch_channel_capacity: 100,
        }
    }
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            failure_threshold: 5,
            failure_window_secs: 10,
            recovery_timeout_secs: 30,
            success_threshold: 2,
        }
    }
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            slow_query_threshold_ms: 100, // 100ms default threshold
            tracing_enabled: false,
            otlp_endpoint: None,
            service_name: "task-runner".to_string(),
            sampling_ratio: 1.0,
        }
    }
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            // In debug builds, skip SSRF validation by default for easier local development
            skip_ssrf_validation: cfg!(debug_assertions),
            blocked_hostnames: vec![
                "localhost".to_string(),
                "127.0.0.1".to_string(),
                "::1".to_string(),
                "0.0.0.0".to_string(),
                "local".to_string(),
                "internal".to_string(),
            ],
            blocked_hostname_suffixes: vec![
                ".local".to_string(),
                ".internal".to_string(),
                ".localdomain".to_string(),
                ".localhost".to_string(),
            ],
        }
    }
}

/// Configuration loading error.
#[derive(Debug)]
pub struct ConfigError {
    pub field: String,
    pub message: String,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Configuration error for '{}': {}",
            self.field, self.message
        )
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// Load configuration from environment variables.
    ///
    /// Required environment variables:
    /// - `DATABASE_URL`: PostgreSQL connection string
    /// - `HOST_URL`: Public URL for webhook callbacks
    ///
    /// Optional environment variables:
    /// - `PORT`: Server port (default: 8085)
    /// - `POOL_MAX_SIZE`: Max pool connections (default: 10)
    /// - `POOL_MIN_IDLE`: Min idle connections (default: 5)
    /// - `POOL_ACQUIRE_RETRIES`: Connection acquire retries (default: 3)
    /// - `PAGINATION_DEFAULT`: Default items per page (default: 50)
    /// - `PAGINATION_MAX`: Max items per page (default: 100)
    /// - `WORKER_LOOP_INTERVAL_MS`: Worker loop interval in ms (default: 1000)
    /// - `BATCH_CHANNEL_CAPACITY`: Batch update channel size (default: 100)
    /// - `CIRCUIT_BREAKER_ENABLED`: Enable circuit breaker (default: true)
    /// - `CIRCUIT_BREAKER_FAILURE_THRESHOLD`: Failures before opening (default: 5)
    /// - `CIRCUIT_BREAKER_FAILURE_WINDOW_SECS`: Failure counting window (default: 10)
    /// - `CIRCUIT_BREAKER_RECOVERY_TIMEOUT_SECS`: Time before half-open (default: 30)
    /// - `CIRCUIT_BREAKER_SUCCESS_THRESHOLD`: Successes to close (default: 2)
    /// - `SLOW_QUERY_THRESHOLD_MS`: Threshold for slow query warnings in ms (default: 100)
    /// - `TRACING_ENABLED`: Enable distributed tracing (default: false)
    /// - `OTEL_EXPORTER_OTLP_ENDPOINT`: OTLP endpoint URL for trace export
    /// - `OTEL_SERVICE_NAME`: Service name for traces (default: task-runner)
    /// - `OTEL_SAMPLING_RATIO`: Sampling ratio 0.0-1.0 (default: 1.0)
    /// - `SKIP_SSRF_VALIDATION`: Skip SSRF validation (default: 1 in debug, 0 in release)
    /// - `BLOCKED_HOSTNAMES`: Comma-separated list of blocked hostnames (default: localhost,127.0.0.1,::1,0.0.0.0,local,internal)
    /// - `BLOCKED_HOSTNAME_SUFFIXES`: Comma-separated list of blocked hostname suffixes (default: .local,.internal,.localdomain,.localhost)
    pub fn from_env() -> Result<Self, ConfigError> {
        let database_url = std::env::var("DATABASE_URL").map_err(|_| ConfigError {
            field: "DATABASE_URL".to_string(),
            message: "Required environment variable not set".to_string(),
        })?;

        let host_url = std::env::var("HOST_URL").map_err(|_| ConfigError {
            field: "HOST_URL".to_string(),
            message: "Required environment variable not set".to_string(),
        })?;

        let port = parse_env_or("PORT", 8085)?;

        let pool = PoolConfig {
            max_size: parse_env_or("POOL_MAX_SIZE", 10)?,
            min_idle: parse_env_or("POOL_MIN_IDLE", 5)?,
            acquire_retries: parse_env_or("POOL_ACQUIRE_RETRIES", 3)?,
            connection_timeout: Duration::from_secs(parse_env_or("POOL_TIMEOUT_SECS", 30)?),
            ..Default::default()
        };

        let pagination = PaginationConfig {
            default_per_page: parse_env_or("PAGINATION_DEFAULT", 50)?,
            max_per_page: parse_env_or("PAGINATION_MAX", 100)?,
        };

        let worker = WorkerConfig {
            loop_interval: Duration::from_millis(parse_env_or("WORKER_LOOP_INTERVAL_MS", 1000)?),
            batch_channel_capacity: parse_env_or("BATCH_CHANNEL_CAPACITY", 100)?,
            ..Default::default()
        };

        let circuit_breaker = CircuitBreakerConfig {
            enabled: parse_env_or("CIRCUIT_BREAKER_ENABLED", 1)? != 0,
            failure_threshold: parse_env_or("CIRCUIT_BREAKER_FAILURE_THRESHOLD", 5)?,
            failure_window_secs: parse_env_or("CIRCUIT_BREAKER_FAILURE_WINDOW_SECS", 10)?,
            recovery_timeout_secs: parse_env_or("CIRCUIT_BREAKER_RECOVERY_TIMEOUT_SECS", 30)?,
            success_threshold: parse_env_or("CIRCUIT_BREAKER_SUCCESS_THRESHOLD", 2)?,
        };

        let observability = ObservabilityConfig {
            slow_query_threshold_ms: parse_env_or("SLOW_QUERY_THRESHOLD_MS", 100)?,
            tracing_enabled: parse_env_or("TRACING_ENABLED", 0)? != 0,
            otlp_endpoint: std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok(),
            service_name: std::env::var("OTEL_SERVICE_NAME")
                .unwrap_or_else(|_| "task-runner".to_string()),
            sampling_ratio: parse_env_or_f64("OTEL_SAMPLING_RATIO", 1.0)?,
        };

        let security = SecurityConfig {
            // In debug builds default to true, in release default to false
            // Can be overridden via SKIP_SSRF_VALIDATION=1
            skip_ssrf_validation: parse_env_or(
                "SKIP_SSRF_VALIDATION",
                if cfg!(debug_assertions) { 1 } else { 0 },
            )? != 0,
            blocked_hostnames: std::env::var("BLOCKED_HOSTNAMES")
                .map(|s| s.split(',').map(|h| h.trim().to_string()).collect())
                .unwrap_or_else(|_| SecurityConfig::default().blocked_hostnames),
            blocked_hostname_suffixes: std::env::var("BLOCKED_HOSTNAME_SUFFIXES")
                .map(|s| s.split(',').map(|h| h.trim().to_string()).collect())
                .unwrap_or_else(|_| SecurityConfig::default().blocked_hostname_suffixes),
        };

        let config = Self {
            database_url,
            host_url,
            port,
            pool,
            pagination,
            worker,
            circuit_breaker,
            observability,
            security,
        };

        config.validate()?;
        Ok(config)
    }

    /// Validate configuration values.
    fn validate(&self) -> Result<(), ConfigError> {
        if self.database_url.is_empty() {
            return Err(ConfigError {
                field: "DATABASE_URL".to_string(),
                message: "Cannot be empty".to_string(),
            });
        }

        if self.host_url.is_empty() {
            return Err(ConfigError {
                field: "HOST_URL".to_string(),
                message: "Cannot be empty".to_string(),
            });
        }

        if !self.host_url.starts_with("http://") && !self.host_url.starts_with("https://") {
            return Err(ConfigError {
                field: "HOST_URL".to_string(),
                message: "Must start with http:// or https://".to_string(),
            });
        }

        if self.pool.max_size == 0 {
            return Err(ConfigError {
                field: "POOL_MAX_SIZE".to_string(),
                message: "Must be greater than 0".to_string(),
            });
        }

        if self.pool.min_idle > self.pool.max_size {
            return Err(ConfigError {
                field: "POOL_MIN_IDLE".to_string(),
                message: "Cannot be greater than POOL_MAX_SIZE".to_string(),
            });
        }

        if self.pagination.max_per_page == 0 {
            return Err(ConfigError {
                field: "PAGINATION_MAX".to_string(),
                message: "Must be greater than 0".to_string(),
            });
        }

        if self.pagination.default_per_page > self.pagination.max_per_page {
            return Err(ConfigError {
                field: "PAGINATION_DEFAULT".to_string(),
                message: "Cannot be greater than PAGINATION_MAX".to_string(),
            });
        }

        Ok(())
    }
}

/// Parse an environment variable or return a default value.
fn parse_env_or<T: std::str::FromStr>(name: &str, default: T) -> Result<T, ConfigError> {
    match std::env::var(name) {
        Ok(val) => val.parse().map_err(|_| ConfigError {
            field: name.to_string(),
            message: format!("Invalid value '{}', expected a valid number", val),
        }),
        Err(_) => Ok(default),
    }
}

/// Parse a floating-point environment variable or return a default value.
fn parse_env_or_f64(name: &str, default: f64) -> Result<f64, ConfigError> {
    match std::env::var(name) {
        Ok(val) => val.parse().map_err(|_| ConfigError {
            field: name.to_string(),
            message: format!("Invalid value '{}', expected a valid decimal number", val),
        }),
        Err(_) => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_pool_config() {
        let config = PoolConfig::default();
        assert_eq!(config.max_size, 10);
        assert_eq!(config.min_idle, 5);
    }

    #[test]
    fn test_default_pagination_config() {
        let config = PaginationConfig::default();
        assert_eq!(config.default_per_page, 50);
        assert_eq!(config.max_per_page, 100);
    }
}
