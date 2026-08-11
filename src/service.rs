use crate::error::PwpError;
use crate::manager::PwpManager;
use actix_web::{HttpRequest, HttpResponse, Responder, web};
use log::info;

pub async fn handle_request(
    request: HttpRequest,
    manager: web::Data<PwpManager>,
) -> Result<impl Responder, PwpError> {
    info!(
        "Received request: {} {} {:?}",
        request.method(),
        request.path(),
        request.version()
    );

    let manager = manager.into_inner();

    let proxy_target = manager.clone().choose_proxy_target(&request)?;
    manager
        .clone()
        .ensure_requested_vm_state_for_target(proxy_target.clone())
        .await?;

    let proxy_url = manager.choose_proxy_url(&request, proxy_target)?;

    if let Some(proxy_url) = proxy_url {
        // TODO: Start proxy connection
        Ok(HttpResponse::Found().body(proxy_url.to_string()))
    } else {
        Ok(HttpResponse::NoContent().finish())
    }
}
