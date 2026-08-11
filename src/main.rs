use actix_settings::{ApplySettings, BasicSettings};
use actix_web::middleware::{Compress, Condition, Logger};
use actix_web::{App, HttpServer, web};
use log::info;
use proxmox_wol_proxy::error::PwpError;
use proxmox_wol_proxy::manager::PwpManager;
use proxmox_wol_proxy::service;
use proxmox_wol_proxy::settings::PwpSettings;

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
