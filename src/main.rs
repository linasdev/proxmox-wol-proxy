use actix_settings::{ApplySettings, BasicSettings};
use actix_web::middleware::{Compress, Condition, Logger};
use actix_web::{App, HttpServer, web};
use log::info;
use proxmox_wol_proxy::service;
use proxmox_wol_proxy::settings::PwpSettings;
use std::io;

#[actix_web::main]
async fn main() -> io::Result<()> {
    env_logger::init();

    let settings =
        BasicSettings::<PwpSettings>::parse_toml("./pwp.toml").expect("Failed to parse config");

    info!(
        "Config loaded, starting Proxmox WoL proxy v{}",
        env!("CARGO_PKG_VERSION")
    );

    HttpServer::new({
        let settings = settings.clone();
        move || {
            App::new()
                .wrap(Condition::new(
                    settings.actix.enable_compression,
                    Compress::default(),
                ))
                .wrap(Logger::default())
                .app_data(web::Data::new(settings.application.clone()))
                .default_service(web::to(service::handle_request))
        }
    })
    .try_apply_settings(&settings.clone())?
    .run()
    .await
}
