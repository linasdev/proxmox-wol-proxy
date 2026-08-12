use actix_web::body::BoxBody;
use actix_web::http::StatusCode;
use actix_web::http::header::{ContentType, ToStrError};
use actix_web::{HttpResponse, ResponseError};
use log::warn;
use std::fmt::{Display, Formatter};
use std::net::AddrParseError;
use std::sync::Arc;
use tokio::io;

#[derive(Debug, Clone)]
pub enum PwpError {
    IO(Arc<io::Error>),
    ActixSettings(Arc<actix_settings::Error>),
    SerdeJson(Arc<serde_json::Error>),
    ProxmoxClient(Arc<proxmox_client::Error>),
    InvalidProxyTargetHeader(Arc<ToStrError>),
    InvalidProxyUrlHeader(Arc<ToStrError>),
    MissingProxyTarget(String),
    DuplicateProxyTargetName(String),
    InvalidProxyTargetUrl(url::ParseError),
    InvalidHeaderUrl(url::ParseError),
    InvalidMacAddress(macaddr::ParseError),
    InvalidBroadcastAddress(AddrParseError),
    InvalidTrustedProxyAddress(AddrParseError),
    MissingProxyTargetUrlHost,
    MissingProxyUrl,
    MissingOrDuplicateProxyTargetHeader,
    FailedToAssembleProxyUrl,
    ProxyError,
    ProxmoxNodeStartTimedOut,
    ProxmoxNodeBusy,
    ProxmoxVmStartTimedOut,
    AccessDenied,
}

impl Display for PwpError {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            PwpError::IO(error) => write!(fmt, "IO error: {error}"),
            PwpError::ActixSettings(error) => write!(fmt, "Actix settings error: {error}"),
            PwpError::SerdeJson(error) => write!(fmt, "Serde JSON error: {error}"),
            PwpError::ProxmoxClient(error) => write!(fmt, "Proxmox client error: {error}"),
            PwpError::InvalidProxyTargetHeader(error) => {
                write!(fmt, "Invalid proxy target header: {error}")
            }
            PwpError::InvalidProxyUrlHeader(error) => {
                write!(fmt, "Invalid proxy URL header: {error}")
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
            PwpError::InvalidMacAddress(error) => write!(fmt, "Invalid MAC address: {error}"),
            PwpError::InvalidBroadcastAddress(error) => {
                write!(fmt, "Invalid broadcast address: {error}")
            }
            PwpError::InvalidTrustedProxyAddress(error) => {
                write!(fmt, "Invalid trusted proxy address: {error}")
            }
            PwpError::MissingProxyTargetUrlHost => write!(fmt, "Missing proxy target URL host"),
            PwpError::MissingProxyUrl => write!(fmt, "Missing proxy URL"),
            PwpError::MissingOrDuplicateProxyTargetHeader => {
                write!(fmt, "Missing or duplicate proxy target header")
            }
            PwpError::FailedToAssembleProxyUrl => write!(fmt, "Failed to assemble proxy URL"),
            PwpError::ProxyError => write!(fmt, "Proxy error"),
            PwpError::ProxmoxNodeStartTimedOut => write!(fmt, "Proxmox node start timed out"),
            PwpError::ProxmoxNodeBusy => write!(fmt, "Proxmox node is busy"),
            PwpError::ProxmoxVmStartTimedOut => write!(fmt, "Proxmox VM start timed out"),
            PwpError::AccessDenied => write!(fmt, "Access denied"),
        }
    }
}

impl ResponseError for PwpError {
    fn error_response(&self) -> HttpResponse<BoxBody> {
        warn!("Sending response after error: {self}");

        match self {
            PwpError::DuplicateProxyTargetName(_)
            | PwpError::InvalidProxyTargetUrl(_)
            | PwpError::InvalidMacAddress(_)
            | PwpError::InvalidBroadcastAddress(_)
            | PwpError::MissingProxyTargetUrlHost => {
                HttpResponse::build(StatusCode::UNPROCESSABLE_ENTITY)
                    .insert_header(ContentType::plaintext())
                    .body("Server is misconfigured")
            }
            PwpError::InvalidProxyTargetHeader(_) => HttpResponse::build(StatusCode::BAD_REQUEST)
                .insert_header(ContentType::plaintext())
                .body(
                    "Invalid proxy target header format, expected 'X-Proxy-Target: <proxy-target>'",
                ),
            PwpError::InvalidProxyUrlHeader(_) => HttpResponse::build(StatusCode::BAD_REQUEST)
                .insert_header(ContentType::plaintext())
                .body("Invalid proxy URL header format, expected 'X-Proxy-URL: <proxy-url>'"),
            PwpError::InvalidHeaderUrl(_) => HttpResponse::build(StatusCode::BAD_REQUEST)
                .insert_header(ContentType::plaintext())
                .body("Invalid proxy URL header format, expected 'X-Proxy-URL: <proxy-url>'"),
            PwpError::MissingOrDuplicateProxyTargetHeader => {
                HttpResponse::build(StatusCode::BAD_REQUEST)
                    .insert_header(ContentType::plaintext())
                    .body("Missing or duplicate 'X-Proxy-Target' header")
            }
            PwpError::MissingProxyTarget(_) => HttpResponse::build(StatusCode::NOT_FOUND)
                .insert_header(ContentType::plaintext())
                .body("Proxy target not found"),
            PwpError::MissingProxyUrl => HttpResponse::build(StatusCode::NOT_FOUND)
                .insert_header(ContentType::plaintext())
                .body("Proxy URL not found"),
            PwpError::ProxmoxNodeStartTimedOut => HttpResponse::build(StatusCode::GATEWAY_TIMEOUT)
                .insert_header(ContentType::plaintext())
                .body("Proxmox node start timed out"),
            PwpError::ProxmoxVmStartTimedOut => HttpResponse::build(StatusCode::GATEWAY_TIMEOUT)
                .insert_header(ContentType::plaintext())
                .body("Proxmox VM start timed out"),
            PwpError::ProxmoxNodeBusy => HttpResponse::build(StatusCode::SERVICE_UNAVAILABLE)
                .insert_header(ContentType::plaintext())
                .body("Proxmox node is busy"),
            PwpError::AccessDenied => HttpResponse::build(StatusCode::FORBIDDEN)
                .insert_header(ContentType::plaintext())
                .body("Forbidden"),
            _ => HttpResponse::build(StatusCode::INTERNAL_SERVER_ERROR)
                .insert_header(ContentType::plaintext())
                .body("Internal server error"),
        }
    }
}

impl From<io::Error> for PwpError {
    fn from(error: io::Error) -> Self {
        PwpError::IO(Arc::new(error))
    }
}

impl From<actix_settings::Error> for PwpError {
    fn from(error: actix_settings::Error) -> Self {
        PwpError::ActixSettings(Arc::new(error))
    }
}

impl From<serde_json::Error> for PwpError {
    fn from(error: serde_json::Error) -> Self {
        PwpError::SerdeJson(Arc::new(error))
    }
}

impl From<proxmox_client::Error> for PwpError {
    fn from(error: proxmox_client::Error) -> Self {
        PwpError::ProxmoxClient(Arc::new(error))
    }
}
