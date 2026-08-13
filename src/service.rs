use crate::error::PwpError;
use crate::manager::PwpManager;
use actix_web::{HttpRequest, HttpResponse, Responder, web};
use log::info;

pub async fn handle_request(
    request: HttpRequest,
    payload: web::Payload,
    manager: web::Data<PwpManager>,
) -> Result<impl Responder, PwpError> {
    info!(
        "Received request: {} {} {:?}",
        request.method(),
        request.path(),
        request.version()
    );

    let manager = manager.into_inner();

    let peer_address = request.peer_addr();

    manager.clone().authenticate(&peer_address)?;

    let proxy_target = manager.clone().choose_proxy_target(&request)?;
    manager
        .clone()
        .ensure_requested_vm_state_for_target(proxy_target.clone())
        .await?;

    let proxy_uri = manager.choose_proxy_uri(&request, proxy_target.clone())?;

    if let Some(proxy_uri) = proxy_uri {
        proxy_target
            .proxy(proxy_uri, payload, request.head(), &peer_address)
            .await
    } else {
        Ok(HttpResponse::NoContent().finish())
    }
}
