use crate::error::PwpError;
use crate::manager::PwpManager;
use crate::settings::PwpSettings;
use axum::Router;
use axum_server::tls_rustls::RustlsConfig;
use log::info;
use reqwest::Client;
use rustls::{ClientConfig, RootCertStore};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use tokio::select;
use tokio::task::spawn_blocking;
use tokio::time::Duration;
use tokio_util::sync::CancellationToken;

pub mod error;
pub mod manager;
pub mod proxmox;
pub mod service;
pub mod settings;
pub mod target;

#[derive(Clone)]
pub struct PwpState {
    pub proxy_target_client: Client,
    pub manager: Arc<PwpManager>,
}

#[tokio::main]
async fn main() -> Result<(), PwpError> {
    env_logger::init();

    let settings: PwpSettings = config::Config::builder()
        .add_source(config::File::with_name("pwp.toml"))
        .add_source(config::Environment::with_prefix("PWP"))
        .build()
        .map_err(Arc::new)
        .map_err(PwpError::Config)?
        .try_deserialize()
        .map_err(Arc::new)
        .map_err(PwpError::Config)?;

    info!(
        "Config loaded, starting Proxmox WoL proxy v{}",
        env!("CARGO_PKG_VERSION")
    );

    let cancellation_token = CancellationToken::new();
    let manager = PwpManager::new(settings.clone())?;
    let vm_shutdown_handler_join_handle = tokio::spawn({
        let manager = manager.clone();
        let cancellation_token = cancellation_token.clone();
        async move {
            select! {
                _ = manager.handle_vm_shutdowns() => {},
                _ = cancellation_token.cancelled() => info!("Stopping VM shutdown handler"),
            }
        }
    });

    let proxy_target_client = {
        let proxy_target_client_config =
            if let Some(root_certificate_path) = settings.client.tls_root_certificate_path {
                let root_certificate_store =
                    spawn_blocking(move || load_root_certificate_store(&root_certificate_path))
                        .await
                        .expect("Failed to spawn blocking task")?;

                let client_config = ClientConfig::builder()
                    .with_root_certificates(root_certificate_store)
                    .with_no_client_auth();

                Some(client_config)
            } else {
                None
            };

        let proxy_target_client_connect_timeout = settings
            .client
            .proxy_client_connect_timeout_ms
            .map(Duration::from_millis);

        let proxy_target_client_timeout = settings
            .client
            .proxy_client_timeout_ms
            .map(Duration::from_millis);

        let mut proxy_target_client_builder = Client::builder();

        if let Some(client_config) = proxy_target_client_config {
            proxy_target_client_builder =
                proxy_target_client_builder.use_preconfigured_tls(client_config);
        }

        if let Some(proxy_target_client_connect_timeout) = proxy_target_client_connect_timeout {
            proxy_target_client_builder =
                proxy_target_client_builder.connect_timeout(proxy_target_client_connect_timeout);
        }

        if let Some(proxy_target_client_timeout) = proxy_target_client_timeout {
            proxy_target_client_builder =
                proxy_target_client_builder.timeout(proxy_target_client_timeout);
        }

        proxy_target_client_builder
            .build()
            .map_err(Arc::new)
            .map_err(PwpError::ProxyClientError)?
    };

    let server_future = async move {
        let app = Router::new()
            .fallback(service::handle_request)
            .with_state(PwpState {
                proxy_target_client,
                manager,
            });
        let address = SocketAddr::from_str(
            format!(
                "{}:{}",
                settings.server.listen_address, settings.server.listen_port
            )
            .as_str(),
        )
        .map_err(PwpError::InvalidListenAddress)?;

        match (
            settings.server.tls_certificate_chain_path,
            settings.server.tls_private_key_path,
        ) {
            (Some(tls_certificate_chain_path), Some(tls_private_key_path)) => {
                let tls_config = RustlsConfig::from_pem_chain_file(
                    tls_certificate_chain_path,
                    tls_private_key_path,
                )
                .await?;

                axum_server::bind_rustls(address, tls_config)
                    .serve(app.into_make_service_with_connect_info::<SocketAddr>())
                    .await?
            }
            (None, None) => {
                axum_server::bind(address)
                    .serve(app.into_make_service_with_connect_info::<SocketAddr>())
                    .await?
            }
            _ => return Err(PwpError::IncompleteServerTlsDetails),
        }

        Ok(())
    };

    server_future.await?;
    cancellation_token.cancel();
    vm_shutdown_handler_join_handle
        .await
        .expect("Failed to join with VM shutdown handler task");

    Ok(())
}

fn load_root_certificate_store(path: &PathBuf) -> Result<RootCertStore, PwpError> {
    let file = std::fs::File::open(path)?;
    let mut buffer_reader = std::io::BufReader::new(file);
    let root_certificates =
        rustls_pemfile::certs(&mut buffer_reader).collect::<Result<Vec<_>, _>>()?;

    let mut root_certificate_store = RootCertStore::empty();
    root_certificate_store.add_parsable_certificates(root_certificates.into_iter());

    Ok(root_certificate_store)
}
