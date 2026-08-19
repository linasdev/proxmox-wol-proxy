use axum::http::StatusCode;
use axum::http::header::ToStrError;
use axum::response::{IntoResponse, Response};
use config::ConfigError;
use log::warn;
use std::fmt::{Display, Formatter};
use std::net::AddrParseError;
use std::sync::Arc;
use tokio::io;

#[derive(Debug, Clone)]
pub enum PwpError {
    Config(Arc<ConfigError>),
    IO(Arc<io::Error>),
    SerdeJson(Arc<serde_json::Error>),
    InvalidProxyTargetHeader(Arc<ToStrError>),
    InvalidProxyUrlHeader(Arc<ToStrError>),
    AxumHttpError(Arc<axum::http::Error>),
    ProxyClientError(Arc<reqwest::Error>),
    ProxmoxClientError(Arc<reqwest::Error>),
    NonSuccessProxmoxResponse(StatusCode, Option<String>),
    InvalidProxmoxResponse(String),
    MissingProxyTarget(String),
    DuplicateProxyTargetName(String),
    InvalidProxyTargetUrl(url::ParseError),
    InvalidHeaderUrl(url::ParseError),
    InvalidRequestUrl(url::ParseError),
    InvalidMacAddress(macaddr::ParseError),
    InvalidListenAddress(AddrParseError),
    InvalidBroadcastAddress(AddrParseError),
    InvalidTrustedProxyAddress(AddrParseError),
    InvalidCertificateFingerprint(hex::FromHexError),
    InvalidProxmoxAuthorizationHeader,
    IncompleteServerTlsDetails,
    FailedToLoadPrivateKey,
    MissingProxyTargetUrlHost,
    MissingProxyUrl,
    MissingOrDuplicateProxyTargetHeader,
    FailedToAssembleProxyUrl,
    ProxmoxNodeStartTimedOut,
    ProxmoxNodeBusy,
    ProxmoxVmStartTimedOut,
    AccessDenied,
}

impl Display for PwpError {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            PwpError::Config(error) => write!(fmt, "Config error: {error}"),
            PwpError::IO(error) => write!(fmt, "IO error: {error}"),
            PwpError::SerdeJson(error) => write!(fmt, "Serde JSON error: {error}"),
            PwpError::InvalidProxyTargetHeader(error) => {
                write!(fmt, "Invalid proxy target header: {error}")
            }
            PwpError::InvalidProxyUrlHeader(error) => {
                write!(fmt, "Invalid proxy URL header: {error}")
            }
            PwpError::AxumHttpError(error) => write!(fmt, "Axum http error: {error}"),
            PwpError::ProxyClientError(error) => write!(fmt, "Proxy client error: {error}"),
            PwpError::ProxmoxClientError(error) => write!(fmt, "Proxmox client error: {error}"),
            PwpError::NonSuccessProxmoxResponse(status_code, body) => write!(
                fmt,
                "Non-success Proxmox response with status '{status_code}' and body: {body:?}"
            ),
            PwpError::InvalidProxmoxResponse(body) => {
                write!(fmt, "Invalid Proxmox response: {body}")
            }
            PwpError::MissingProxyTarget(name) => {
                write!(fmt, "Missing proxy target with name: {name}")
            }
            PwpError::DuplicateProxyTargetName(name) => {
                write!(fmt, "Duplicate proxy target name: {name}")
            }
            PwpError::InvalidProxyTargetUrl(error) => {
                write!(fmt, "Invalid proxy target URL: {error}")
            }
            PwpError::InvalidHeaderUrl(error) => write!(fmt, "Invalid header URL: {error}"),
            PwpError::InvalidRequestUrl(error) => write!(fmt, "Invalid request URL: {error}"),
            PwpError::InvalidMacAddress(error) => write!(fmt, "Invalid MAC address: {error}"),
            PwpError::InvalidListenAddress(error) => {
                write!(fmt, "Invalid listen address: {error}")
            }
            PwpError::InvalidBroadcastAddress(error) => {
                write!(fmt, "Invalid broadcast address: {error}")
            }
            PwpError::InvalidTrustedProxyAddress(error) => {
                write!(fmt, "Invalid trusted proxy address: {error}")
            }
            PwpError::InvalidCertificateFingerprint(error) => {
                write!(fmt, "Invalid certificate fingerprint: {error}")
            }
            PwpError::InvalidProxmoxAuthorizationHeader => {
                write!(fmt, "Invalid Proxmox authorization header")
            }
            PwpError::IncompleteServerTlsDetails => write!(fmt, "Incomplete server TLS details"),
            PwpError::FailedToLoadPrivateKey => write!(fmt, "Failed to load private key"),
            PwpError::MissingProxyTargetUrlHost => write!(fmt, "Missing proxy target URL host"),
            PwpError::MissingProxyUrl => write!(fmt, "Missing proxy URL"),
            PwpError::MissingOrDuplicateProxyTargetHeader => {
                write!(fmt, "Missing or duplicate proxy target header")
            }
            PwpError::FailedToAssembleProxyUrl => write!(fmt, "Failed to assemble proxy URL"),
            PwpError::ProxmoxNodeStartTimedOut => write!(fmt, "Proxmox node start timed out"),
            PwpError::ProxmoxNodeBusy => write!(fmt, "Proxmox node is busy"),
            PwpError::ProxmoxVmStartTimedOut => write!(fmt, "Proxmox VM start timed out"),
            PwpError::AccessDenied => write!(fmt, "Access denied"),
        }
    }
}

impl From<io::Error> for PwpError {
    fn from(error: io::Error) -> Self {
        PwpError::IO(Arc::new(error))
    }
}

impl From<serde_json::Error> for PwpError {
    fn from(error: serde_json::Error) -> Self {
        PwpError::SerdeJson(Arc::new(error))
    }
}

impl IntoResponse for PwpError {
    fn into_response(self) -> Response {
        warn!("Sending response after error: {self}");

        match self {
            PwpError::DuplicateProxyTargetName(_)
            | PwpError::InvalidProxyTargetUrl(_)
            | PwpError::InvalidMacAddress(_)
            | PwpError::InvalidBroadcastAddress(_)
            | PwpError::MissingProxyTargetUrlHost => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "Server is misconfigured".to_string(),
            ),
            PwpError::InvalidProxyTargetHeader(_) => (
                StatusCode::BAD_REQUEST,
                "Invalid proxy target header format, expected 'X-Proxy-Target: <proxy-target>'"
                    .to_string(),
            ),
            PwpError::InvalidProxyUrlHeader(_) => (
                StatusCode::BAD_REQUEST,
                "Invalid proxy URL header format, expected 'X-Proxy-URL: <proxy-url>'".to_string(),
            ),
            PwpError::InvalidHeaderUrl(_) => (
                StatusCode::BAD_REQUEST,
                "Invalid proxy URL header format, expected 'X-Proxy-URL: <proxy-url>'".to_string(),
            ),
            PwpError::MissingOrDuplicateProxyTargetHeader => (
                StatusCode::BAD_REQUEST,
                "Missing or duplicate 'X-Proxy-Target' header".to_string(),
            ),
            PwpError::MissingProxyTarget(_) => {
                (StatusCode::NOT_FOUND, "Proxy target not found".to_string())
            }
            PwpError::MissingProxyUrl => (StatusCode::NOT_FOUND, "Proxy URL not found".to_string()),
            PwpError::ProxmoxNodeStartTimedOut => (
                StatusCode::GATEWAY_TIMEOUT,
                "Proxmox node start timed out".to_string(),
            ),
            PwpError::ProxmoxVmStartTimedOut => (
                StatusCode::GATEWAY_TIMEOUT,
                "Proxmox VM start timed out".to_string(),
            ),
            PwpError::ProxmoxNodeBusy => (
                StatusCode::SERVICE_UNAVAILABLE,
                "Proxmox node is busy".to_string(),
            ),
            PwpError::AccessDenied => (StatusCode::FORBIDDEN, "Forbidden".to_string()),
            _ => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal server error".to_string(),
            ),
        }
        .into_response()
    }
}
