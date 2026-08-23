use std::collections::HashSet;

use axum::http::{Method, Uri};

use crate::config::valid_container_name;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Operation {
    Ping,
    Version,
    ListContainers,
    InspectContainer,
    ContainerStats,
}

pub(crate) fn classify(
    method: &Method,
    uri: &Uri,
    allowed_containers: &HashSet<String>,
) -> Result<Operation, PolicyError> {
    if method != Method::GET && !(method == Method::HEAD && uri.path().ends_with("/_ping")) {
        return Err(PolicyError::MethodNotAllowed);
    }
    if uri.path().contains('%') || uri.path().contains("//") {
        return Err(PolicyError::UnknownEndpoint);
    }

    let path = without_api_version(uri.path()).ok_or(PolicyError::UnknownEndpoint)?;
    match path {
        "/_ping" if uri.query().is_none() => Ok(Operation::Ping),
        "/version" if uri.query().is_none() => Ok(Operation::Version),
        "/containers/json" if list_query_allowed(uri.query()) => Ok(Operation::ListContainers),
        _ => container_operation(path, uri.query(), allowed_containers),
    }
}

fn container_operation(
    path: &str,
    query: Option<&str>,
    allowed: &HashSet<String>,
) -> Result<Operation, PolicyError> {
    let rest = path
        .strip_prefix("/containers/")
        .ok_or(PolicyError::UnknownEndpoint)?;
    let (name, action) = rest.split_once('/').ok_or(PolicyError::UnknownEndpoint)?;
    if !valid_container_name(name) || !allowed.contains(name) {
        return Err(PolicyError::UnknownEndpoint);
    }

    match (action, query) {
        ("json", None) => Ok(Operation::InspectContainer),
        ("stats", Some(query)) if stats_query_allowed(query) => Ok(Operation::ContainerStats),
        _ => Err(PolicyError::UnknownEndpoint),
    }
}

fn without_api_version(path: &str) -> Option<&str> {
    let Some(rest) = path.strip_prefix("/v1.") else {
        return Some(path);
    };
    let slash = rest.find('/')?;
    let version = &rest[..slash];
    if version.is_empty() || !version.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(&rest[slash..])
}

fn list_query_allowed(query: Option<&str>) -> bool {
    matches!(query, Some("all=true" | "all=1"))
}

fn stats_query_allowed(query: &str) -> bool {
    let mut saw_stream = false;
    let mut saw_one_shot = false;

    for pair in query.split('&') {
        match pair {
            "stream=false" | "stream=0" if !saw_stream => saw_stream = true,
            "one-shot=true" | "one-shot=1" if !saw_one_shot => saw_one_shot = true,
            _ => return false,
        }
    }
    saw_stream
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PolicyError {
    #[error("method is not allowed")]
    MethodNotAllowed,
    #[error("endpoint is not available")]
    UnknownEndpoint,
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use axum::http::{Method, Uri};

    use super::{Operation, classify};

    fn allowed() -> HashSet<String> {
        ["homepage", "grafana"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    fn classify_uri(method: Method, uri: &str) -> Result<Operation, super::PolicyError> {
        classify(&method, &uri.parse::<Uri>().unwrap(), &allowed())
    }

    #[test]
    fn permits_the_homepage_v2_1_2_non_swarm_contract() {
        assert_eq!(
            classify_uri(Method::GET, "/containers/json?all=true").unwrap(),
            Operation::ListContainers
        );
        assert_eq!(
            classify_uri(Method::GET, "/containers/homepage/json").unwrap(),
            Operation::InspectContainer
        );
        assert_eq!(
            classify_uri(Method::GET, "/containers/homepage/stats?stream=false").unwrap(),
            Operation::ContainerStats
        );
    }

    #[test]
    fn accepts_docker_api_version_prefixes() {
        assert_eq!(
            classify_uri(Method::GET, "/v1.52/containers/json?all=1").unwrap(),
            Operation::ListContainers
        );
    }

    #[test]
    fn rejects_every_mutating_method() {
        for method in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
            assert!(classify_uri(method, "/containers/homepage/json").is_err());
        }
    }

    #[test]
    fn rejects_powerful_read_endpoints() {
        for uri in [
            "/containers/homepage/logs?stdout=1",
            "/containers/homepage/archive?path=/",
            "/images/json",
            "/volumes",
            "/info",
        ] {
            assert!(classify_uri(Method::GET, uri).is_err(), "accepted {uri}");
        }
    }

    #[test]
    fn rejects_unlisted_containers_and_streaming_stats() {
        assert!(classify_uri(Method::GET, "/containers/database/json").is_err());
        assert!(classify_uri(Method::GET, "/containers/homepage/stats?stream=true").is_err());
        assert!(classify_uri(Method::GET, "/containers/homepage/stats").is_err());
    }

    #[test]
    fn rejects_path_smuggling_and_extra_query_parameters() {
        assert!(classify_uri(Method::GET, "/containers/%2e%2e/json").is_err());
        assert!(classify_uri(Method::GET, "/containers//json").is_err());
        assert!(classify_uri(Method::GET, "/containers/json?all=true&size=true").is_err());
    }
}
