use crate::error::PwpError;
use crate::settings::PwpSettings;
use crate::target::settings::PwpTargetSettings;
use actix_web::http::header::HeaderMap;
use actix_web::{HttpRequest, HttpResponse, Responder, web};
use url::Url;

pub async fn handle_request(
    request: HttpRequest,
    settings: web::Data<PwpSettings>,
) -> Result<impl Responder, PwpError> {
    let proxy_target = choose_proxy_target(request.headers(), &settings.targets)?;

    let proxy_url = proxy_target
        .proxy_url
        .as_ref()
        .map(|target_url| {
            let target_url = Url::parse(target_url).map_err(PwpError::InvalidTargetUrl)?;
            let proxy_url = assemble_proxy_url(target_url, request.full_url().clone())?;
            Ok(proxy_url)
        })
        .transpose()?;

    if let Some(proxy_url) = proxy_url.as_ref() {
        Ok(HttpResponse::Found().body(proxy_url.to_string()))
    } else {
        Ok(HttpResponse::NoContent().finish())
    }
}

fn choose_proxy_target(
    headers: &HeaderMap,
    target_settings: &[PwpTargetSettings],
) -> Result<PwpTargetSettings, PwpError> {
    let target_headers = headers.get_all("X-Proxy-Target").collect::<Vec<_>>();
    if target_headers.len() != 1 {
        return Err(PwpError::MissingOrDuplicateTargetHeader);
    }

    let target_header = target_headers[0]
        .to_str()
        .map_err(PwpError::InvalidTargetHeader)?;

    if target_header.trim().is_empty() {
        return Err(PwpError::MissingOrDuplicateTargetHeader);
    }

    let mut chosen_target_settings = None;

    for current_target_settings in target_settings.iter() {
        if current_target_settings.name.as_str() == target_header {
            if chosen_target_settings.is_some() {
                return Err(PwpError::DuplicateTargetName(target_header.to_string()));
            }

            chosen_target_settings = Some(current_target_settings.clone());
        }
    }

    chosen_target_settings.ok_or_else(|| PwpError::MissingTargetSettings(target_header.to_string()))
}

fn assemble_proxy_url(target_url: Url, request_url: Url) -> Result<Url, PwpError> {
    let mut proxy_url = request_url;
    proxy_url
        .set_scheme(target_url.scheme())
        .map_err(|_| PwpError::FailedToAssembleProxyUrl)?;
    proxy_url
        .set_host(target_url.host_str())
        .map_err(|_| PwpError::FailedToAssembleProxyUrl)?;
    proxy_url
        .set_port(target_url.port())
        .map_err(|_| PwpError::FailedToAssembleProxyUrl)?;

    Ok(proxy_url)
}
