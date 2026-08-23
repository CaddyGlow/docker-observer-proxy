use std::{
    collections::HashSet,
    env, fs,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    time::Duration,
};

#[derive(Clone, Debug)]
pub struct Config {
    pub listen_addr: SocketAddr,
    pub docker_socket: PathBuf,
    pub auth_token: String,
    pub allowed_client_ips: HashSet<IpAddr>,
    pub allowed_containers: HashSet<String>,
    pub timeout: Duration,
    pub max_response_bytes: usize,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        let listen_addr = value("DOP_LISTEN_ADDR", "127.0.0.1:2375")
            .parse()
            .map_err(|_| ConfigError::Invalid("DOP_LISTEN_ADDR"))?;
        let docker_socket = PathBuf::from(value("DOP_DOCKER_SOCKET", "/var/run/docker.sock"));
        let auth_token = read_token()?;
        if auth_token.len() < 32 {
            return Err(ConfigError::TokenTooShort);
        }

        let allowed_client_ips = list("DOP_ALLOWED_CLIENT_IPS", "127.0.0.1,::1")
            .map(|item| {
                item.parse()
                    .map_err(|_| ConfigError::Invalid("DOP_ALLOWED_CLIENT_IPS"))
            })
            .collect::<Result<HashSet<_>, _>>()?;
        let allowed_containers = list("DOP_ALLOWED_CONTAINERS", "").collect::<HashSet<_>>();
        if allowed_containers.is_empty() {
            return Err(ConfigError::Missing("DOP_ALLOWED_CONTAINERS"));
        }
        if allowed_containers
            .iter()
            .any(|name| !valid_container_name(name))
        {
            return Err(ConfigError::Invalid("DOP_ALLOWED_CONTAINERS"));
        }

        let timeout = Duration::from_secs(parse("DOP_TIMEOUT_SECONDS", 5_u64)?);
        let max_response_bytes = parse("DOP_MAX_RESPONSE_BYTES", 8 * 1024 * 1024_usize)?;
        if timeout.is_zero() || max_response_bytes == 0 {
            return Err(ConfigError::Invalid("timeout or response limit"));
        }

        Ok(Self {
            listen_addr,
            docker_socket,
            auth_token,
            allowed_client_ips,
            allowed_containers,
            timeout,
            max_response_bytes,
        })
    }
}

fn read_token() -> Result<String, ConfigError> {
    match (
        env::var("DOP_AUTH_TOKEN").ok(),
        env::var("DOP_AUTH_TOKEN_FILE").ok(),
    ) {
        (Some(_), Some(_)) => Err(ConfigError::AmbiguousToken),
        (Some(token), None) => Ok(token),
        (None, Some(path)) => fs::read_to_string(path)
            .map(|token| token.trim_end_matches(['\r', '\n']).to_owned())
            .map_err(ConfigError::TokenFile),
        (None, None) => Err(ConfigError::Missing(
            "DOP_AUTH_TOKEN or DOP_AUTH_TOKEN_FILE",
        )),
    }
}

fn value(name: &'static str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_owned())
}

fn list(name: &'static str, default: &'static str) -> impl Iterator<Item = String> {
    value(name, default)
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>()
        .into_iter()
}

fn parse<T>(name: &'static str, default: T) -> Result<T, ConfigError>
where
    T: std::str::FromStr + ToString,
{
    value(name, &default.to_string())
        .parse()
        .map_err(|_| ConfigError::Invalid(name))
}

pub(crate) fn valid_container_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("missing required setting {0}")]
    Missing(&'static str),
    #[error("invalid value for {0}")]
    Invalid(&'static str),
    #[error("set only one of DOP_AUTH_TOKEN and DOP_AUTH_TOKEN_FILE")]
    AmbiguousToken,
    #[error("authentication token must contain at least 32 bytes")]
    TokenTooShort,
    #[error("cannot read authentication token file: {0}")]
    TokenFile(std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::valid_container_name;

    #[test]
    fn container_names_are_intentionally_boring() {
        assert!(valid_container_name("home-assistant_1"));
        assert!(!valid_container_name("../containers/create"));
        assert!(!valid_container_name("name/child"));
        assert!(!valid_container_name(""));
    }
}
