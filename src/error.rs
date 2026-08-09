use actix_web::body::BoxBody;
use actix_web::http::StatusCode;
use actix_web::http::header::{ContentType, ToStrError};
use actix_web::{HttpResponse, ResponseError};
use log::info;
use std::fmt::{Display, Formatter};

#[derive(Debug)]
pub enum PwpError {
    InvalidTargetHeader(ToStrError),
    MissingTargetSettings(String),
    DuplicateTargetName(String),
    InvalidTargetUrl(url::ParseError),
    MissingOrDuplicateTargetHeader,
    FailedToAssembleProxyUrl,
}

impl Display for PwpError {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            PwpError::InvalidTargetHeader(error) => write!(fmt, "Invalid target header: {error}"),
            PwpError::MissingTargetSettings(name) => {
                write!(fmt, "Missing target settings for: {name}")
            }
            PwpError::DuplicateTargetName(name) => write!(fmt, "Duplicate target name: {name}"),
            PwpError::InvalidTargetUrl(error) => write!(fmt, "Invalid target URL: {error}"),
            PwpError::MissingOrDuplicateTargetHeader => {
                write!(fmt, "Missing or duplicate target header")
            }
            PwpError::FailedToAssembleProxyUrl => write!(fmt, "Failed to assemble proxy URL"),
        }
    }
}

impl ResponseError for PwpError {
    fn error_response(&self) -> HttpResponse<BoxBody> {
        info!("Sending response after error: {self}");

        match self {
            PwpError::MissingTargetSettings(_) => HttpResponse::build(StatusCode::NOT_FOUND)
                .insert_header(ContentType::plaintext())
                .body("Target not found"),
            _ => HttpResponse::build(StatusCode::INTERNAL_SERVER_ERROR)
                .insert_header(ContentType::plaintext())
                .body("Internal server error"),
        }
    }
}
