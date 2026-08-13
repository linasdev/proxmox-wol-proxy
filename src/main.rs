use crate::error::PwpError;
use crate::manager::PwpManager;
use crate::settings::PwpSettings;
use crate::target::PROXY_TARGET_CLIENT;
use actix_settings::{ApplySettings, BasicSettings};
use actix_web::middleware::{Compress, Condition, Logger};
use actix_web::{App, HttpServer, web};
use awc::{Client, Connector};
use log::info;
use rustls::{ClientConfig, RootCertStore};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub mod error;
pub mod manager;
pub mod proxmox;
pub mod service;
pub mod settings;
pub mod target;

#[tokio::main]
async fn main() -> Result<(), PwpError> {
    env_logger::init();

    let settings =
        BasicSettings::<PwpSettings>::parse_toml("./pwp.toml").expect("Failed to parse config");

    info!(
        "Config loaded, starting Proxmox WoL proxy v{}",
        env!("CARGO_PKG_VERSION")
    );

    let manager = PwpManager::new(settings.application.clone())?;

    let client_settings = settings.application.client.clone();

    let proxy_target_client_config: Option<Arc<ClientConfig>> =
        if let Some(root_certificate_path) = client_settings.tls_root_certificate_path.as_ref() {
            let root_certificate_store = load_root_certificate_store(root_certificate_path)?;

            let client_config = ClientConfig::builder()
                .with_root_certificates(root_certificate_store)
                .with_no_client_auth();

            Some(Arc::new(client_config))
        } else {
            None
        };

    let proxy_target_client_connect_timeout = client_settings
        .proxy_client_connect_timeout_ms
        .map(Duration::from_millis);

    let proxy_target_client_timeout = client_settings
        .proxy_client_timeout_ms
        .map(Duration::from_millis);

    HttpServer::new({
        move || {
            PROXY_TARGET_CLIENT.with_borrow_mut(|client_option| {
                let mut connector = Connector::new();

                if let Some(client_config) = proxy_target_client_config.as_ref() {
                    connector = connector.rustls_0_23(client_config.clone());
                }

                if let Some(connect_timeout) = proxy_target_client_connect_timeout {
                    connector = connector.timeout(connect_timeout);
                }

                let mut client_builder = Client::builder().connector(connector);

                if let Some(timeout) = proxy_target_client_timeout {
                    client_builder = client_builder.timeout(timeout);
                }

                client_option.replace(client_builder.finish());
            });

            App::new()
                .wrap(Condition::new(
                    settings.actix.enable_compression,
                    Compress::default(),
                ))
                .wrap(Logger::default())
                .app_data(web::Data::from(manager.clone()))
                .default_service(web::to(service::handle_request))
        }
    })
    .try_apply_settings(&settings)?
    .run()
    .await?;

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
