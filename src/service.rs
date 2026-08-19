use crate::PwpState;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use log::info;
use std::net::SocketAddr;

pub async fn handle_request(
    State(state): State<PwpState>,
    ConnectInfo(peer_address): ConnectInfo<SocketAddr>,
    request: Request,
) -> impl IntoResponse {
    info!(
        "Received request: {} {} {:?}",
        request.method(),
        request.uri(),
        request.version()
    );

    let manager = state.manager;

    manager.clone().authenticate(&peer_address)?;

    let proxy_target = manager.clone().choose_proxy_target(&request)?;
    manager
        .clone()
        .ensure_active_vm_for_target(proxy_target.clone())
        .await?;

    let proxy_url = manager.choose_proxy_url(&request, proxy_target.clone())?;

    if let Some(proxy_url) = proxy_url {
        proxy_target
            .proxy(
                &state.proxy_target_client,
                proxy_url,
                request,
                &peer_address,
            )
            .await
    } else {
        Ok(StatusCode::NO_CONTENT.into_response())
    }
}
