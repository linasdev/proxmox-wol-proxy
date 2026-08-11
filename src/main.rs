use crate::error::PwpError;
use crate::manager::PwpManager;
use crate::settings::PwpSettings;
use crate::target::proxy::PROXY_TARGET_CLIENT;
use actix_settings::{ApplySettings, BasicSettings};
use actix_web::middleware::{Compress, Condition, Logger};
use actix_web::{App, HttpServer, web};
use awc::Client;
use log::info;
use tokio::runtime;

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

    HttpServer::new({
        move || {
            PROXY_TARGET_CLIENT.with_borrow_mut(|client_option| {
                // TODO: TLS
                client_option.replace(Client::default());
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
