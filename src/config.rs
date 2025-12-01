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

/// Configuration loading error.
#[derive(Debug)]
pub struct ConfigError {
    pub field: String,
    pub message: String,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Configuration error for '{}': {}", self.field, self.message)
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

        let config = Self {
            database_url,
            host_url,
            port,
            pool,
            pagination,
            worker,
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
