mod config;
mod policy;
mod upstream;

use std::{
    collections::HashSet,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Instant,
};

use axum::{
    body::{Body, Bytes},
    extract::{ConnectInfo, State},
    http::{Request, Response, StatusCode, header},
    response::IntoResponse,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tracing::{info, warn};

pub use config::{Config, ConfigError};
use policy::{Operation, PolicyError, classify};
use upstream::{UpstreamError, call_docker};

#[derive(Clone)]
pub struct AppState(Arc<InnerState>);

struct InnerState {
    config: Config,
    token_hash: [u8; 32],
}

impl AppState {
    pub fn new(config: Config) -> Self {
        let token_hash = Sha256::digest(config.auth_token.as_bytes()).into();
        Self(Arc::new(InnerState { config, token_hash }))
    }
}

pub async fn health(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Response<Body> {
    if !client_allowed(peer.ip(), &state.0.config.allowed_client_ips) {
        return error_response(StatusCode::FORBIDDEN, "client is not allowed");
    }
    Response::new(Body::from("ok\n"))
}

pub async fn proxy(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request<Body>,
) -> Response<Body> {
    let started = Instant::now();
    let method = request.method().clone();
    let path = request.uri().path().to_owned();

    let result = handle(&state, peer.ip(), request).await;
    let response = match result {
        Ok(response) => response,
        Err(error) => error.into_response(),
    };

    info!(
        client_ip = %peer.ip(),
        method = %method,
        path,
        status = response.status().as_u16(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "request completed"
    );
    response
}

async fn handle(
    state: &AppState,
    client_ip: IpAddr,
    request: Request<Body>,
) -> Result<Response<Body>, ProxyError> {
    if !client_allowed(client_ip, &state.0.config.allowed_client_ips) {
        return Err(ProxyError::Forbidden("client is not allowed"));
    }
    if !token_allowed(
        request.headers().get(header::AUTHORIZATION),
        &state.0.token_hash,
    ) {
        return Err(ProxyError::Unauthorized);
    }

    let operation = classify(
        request.method(),
        request.uri(),
        &state.0.config.allowed_containers,
    )?;
    let path_and_query = request
        .uri()
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or(request.uri().path());

    let mut upstream = call_docker(
        &state.0.config.docker_socket,
        request.method(),
        path_and_query,
        state.0.config.timeout,
        state.0.config.max_response_bytes,
    )
    .await?;

    if operation == Operation::ListContainers && upstream.status.is_success() {
        upstream.body = filter_containers(upstream.body, &state.0.config.allowed_containers)?;
    }

    let mut response = Response::builder().status(upstream.status);
    if let Some(content_type) = upstream.content_type {
        response = response.header(header::CONTENT_TYPE, content_type);
    }
    response
        .body(Body::from(upstream.body))
        .map_err(|_| ProxyError::Upstream(UpstreamError::InvalidResponse))
}

fn client_allowed(client: IpAddr, allowed: &HashSet<IpAddr>) -> bool {
    allowed.contains(&client)
}

fn token_allowed(value: Option<&header::HeaderValue>, expected_hash: &[u8; 32]) -> bool {
    let Some(token) = value
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return false;
    };
    let actual_hash: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    bool::from(actual_hash.ct_eq(expected_hash))
}

fn filter_containers(body: Bytes, allowed: &HashSet<String>) -> Result<Bytes, ProxyError> {
    let mut containers: Vec<Value> = serde_json::from_slice(&body)
        .map_err(|_| ProxyError::BadGateway("Docker returned an invalid container list"))?;

    containers.retain(|container| {
        container
            .get("Names")
            .and_then(Value::as_array)
            .is_some_and(|names| {
                names
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|name| allowed.contains(name.strip_prefix('/').unwrap_or(name)))
            })
    });

    serde_json::to_vec(&containers)
        .map(Bytes::from)
        .map_err(|_| ProxyError::BadGateway("failed to encode filtered container list"))
}

#[derive(Debug, thiserror::Error)]
enum ProxyError {
    #[error("authentication required")]
    Unauthorized,
    #[error("{0}")]
    Forbidden(&'static str),
    #[error("{0}")]
    BadGateway(&'static str),
    #[error(transparent)]
    Policy(#[from] PolicyError),
    #[error(transparent)]
    Upstream(#[from] UpstreamError),
}

impl IntoResponse for ProxyError {
    fn into_response(self) -> Response<Body> {
        let (status, message) = match &self {
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "authentication required"),
            Self::Forbidden(message) => (StatusCode::FORBIDDEN, *message),
            Self::BadGateway(message) => (StatusCode::BAD_GATEWAY, *message),
            Self::Policy(PolicyError::MethodNotAllowed) => {
                (StatusCode::METHOD_NOT_ALLOWED, "method is not allowed")
            }
            Self::Policy(_) => (StatusCode::NOT_FOUND, "endpoint is not available"),
            Self::Upstream(UpstreamError::Timeout) => {
                (StatusCode::GATEWAY_TIMEOUT, "Docker timed out")
            }
            Self::Upstream(_) => (StatusCode::BAD_GATEWAY, "Docker is unavailable"),
        };
        if status.is_server_error() {
            warn!(error = %self, %status, "request failed");
        }
        error_response(status, message)
    }
}

fn error_response(status: StatusCode, message: &'static str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(format!("{message}\n")))
        .expect("static response is valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(values: &[&str]) -> HashSet<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn token_authentication_requires_the_exact_bearer_token() {
        let expected: [u8; 32] = Sha256::digest(b"a-long-secret-token-with-32-bytes").into();
        let valid = header::HeaderValue::from_static("Bearer a-long-secret-token-with-32-bytes");
        let invalid = header::HeaderValue::from_static("Bearer wrong-secret-token-with-32-bytes");

        assert!(token_allowed(Some(&valid), &expected));
        assert!(!token_allowed(Some(&invalid), &expected));
        assert!(!token_allowed(None, &expected));
    }

    #[test]
    fn container_list_is_reduced_to_explicitly_allowed_names() {
        let body = Bytes::from_static(
            br#"[
                {"Id":"one","Names":["/homepage"]},
                {"Id":"two","Names":["/private-database"]},
                {"Id":"three","Names":["/alias","/grafana"]}
            ]"#,
        );
        let filtered = filter_containers(body, &names(&["homepage", "grafana"])).unwrap();
        let value: Vec<Value> = serde_json::from_slice(&filtered).unwrap();

        assert_eq!(value.len(), 2);
        assert_eq!(value[0]["Id"], "one");
        assert_eq!(value[1]["Id"], "three");
    }

    #[test]
    fn malformed_docker_lists_fail_closed() {
        assert!(filter_containers(Bytes::from_static(b"{}"), &names(&["homepage"])).is_err());
    }
}
