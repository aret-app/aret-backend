//! Service settings, read from environment variables (never from files in the repository).

use std::net::SocketAddr;

/// Environment variable that overrides the listen address, e.g. `0.0.0.0:8080` in containers.
pub const HTTP_ADDR_ENV: &str = "ARET_HTTP_ADDR";

/// Settings shared by every service.
#[derive(Debug, Clone, PartialEq)]
pub struct ServiceConfig {
    /// Short service name reported by `/health`, e.g. `identity`.
    pub name: &'static str,
    /// Version reported by `/health`; services pass their crate version.
    pub version: &'static str,
    /// Address the HTTP server listens on.
    pub http_addr: SocketAddr,
}

/// A setting has an invalid value.
#[must_use]
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ConfigError {
    /// The listen address is not of the form `host:port`.
    #[error("{var} must be an address like 0.0.0.0:8080, got {value:?}")]
    InvalidAddr {
        /// Name of the environment variable.
        var: &'static str,
        /// The rejected value.
        value: String,
    },
}

impl ServiceConfig {
    /// Builds the settings from the environment.
    ///
    /// Listens on `127.0.0.1:<default_port>` (this computer only) unless `ARET_HTTP_ADDR` is set;
    /// the Dockerfile sets it to `0.0.0.0:8080`.
    pub fn try_from_env(
        name: &'static str,
        version: &'static str,
        default_port: u16,
    ) -> Result<Self, ConfigError> {
        let http_addr: Option<String> = std::env::var(HTTP_ADDR_ENV).ok();
        Self::try_new(name, version, default_port, http_addr.as_deref())
    }

    /// Builds the settings from explicit values; `http_addr` is the raw `ARET_HTTP_ADDR` value.
    pub fn try_new(
        name: &'static str,
        version: &'static str,
        default_port: u16,
        http_addr: Option<&str>,
    ) -> Result<Self, ConfigError> {
        let http_addr: SocketAddr = match http_addr {
            Some(value) => value.parse().map_err(|_| ConfigError::InvalidAddr {
                var: HTTP_ADDR_ENV,
                value: value.to_owned(),
            })?,
            None => SocketAddr::from(([127, 0, 0, 1], default_port)),
        };
        Ok(Self {
            name,
            version,
            http_addr,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_localhost_on_the_service_port() {
        let config = ServiceConfig::try_new("tracks", "0.1.0", 8082, None).unwrap();
        assert_eq!(config.http_addr, SocketAddr::from(([127, 0, 0, 1], 8082)));
    }

    #[test]
    fn env_value_overrides_the_address() {
        let config = ServiceConfig::try_new("tracks", "0.1.0", 8082, Some("0.0.0.0:8080")).unwrap();
        assert_eq!(config.http_addr, SocketAddr::from(([0, 0, 0, 0], 8080)));
    }

    #[test]
    fn rejects_an_address_without_a_host() {
        let err = ServiceConfig::try_new("tracks", "0.1.0", 8082, Some("8080")).unwrap_err();
        assert_eq!(
            err,
            ConfigError::InvalidAddr {
                var: HTTP_ADDR_ENV,
                value: "8080".to_owned()
            }
        );
    }
}
