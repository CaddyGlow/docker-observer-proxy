use std::{path::Path, time::Duration};

use axum::{
    body::Bytes,
    http::{HeaderValue, Method, Request, StatusCode, header},
};
use http_body_util::{BodyExt, Empty, Limited};
use hyper::client::conn::http1;
use hyper_util::rt::TokioIo;
use tokio::{net::UnixStream, time::timeout};

pub(crate) struct UpstreamResponse {
    pub status: StatusCode,
    pub content_type: Option<HeaderValue>,
    pub body: Bytes,
}

pub(crate) async fn call_docker(
    socket: &Path,
    method: &Method,
    path_and_query: &str,
    deadline: Duration,
    max_response_bytes: usize,
) -> Result<UpstreamResponse, UpstreamError> {
    timeout(deadline, async {
        let stream = UnixStream::connect(socket)
            .await
            .map_err(UpstreamError::Connect)?;
        let (mut sender, connection) = http1::handshake(TokioIo::new(stream))
            .await
            .map_err(UpstreamError::Protocol)?;
        tokio::spawn(async move {
            if let Err(error) = connection.await {
                tracing::debug!(%error, "Docker connection closed");
            }
        });

        let request = Request::builder()
            .method(method)
            .uri(path_and_query)
            .header(header::HOST, "docker")
            .body(Empty::<Bytes>::new())
            .map_err(|_| UpstreamError::InvalidRequest)?;
        let response = sender
            .send_request(request)
            .await
            .map_err(UpstreamError::Protocol)?;
        let status = response.status();
        let content_type = response.headers().get(header::CONTENT_TYPE).cloned();
        let body = Limited::new(response.into_body(), max_response_bytes)
            .collect()
            .await
            .map_err(|_| UpstreamError::ResponseTooLarge)?
            .to_bytes();

        Ok(UpstreamResponse {
            status,
            content_type,
            body,
        })
    })
    .await
    .map_err(|_| UpstreamError::Timeout)?
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum UpstreamError {
    #[error("cannot connect to Docker: {0}")]
    Connect(std::io::Error),
    #[error("Docker protocol error: {0}")]
    Protocol(hyper::Error),
    #[error("invalid upstream request")]
    InvalidRequest,
    #[error("invalid upstream response")]
    InvalidResponse,
    #[error("Docker response exceeded the configured limit")]
    ResponseTooLarge,
    #[error("Docker request timed out")]
    Timeout,
}
