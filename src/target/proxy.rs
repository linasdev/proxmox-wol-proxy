use crate::target::settings::PwpTargetSettings;
use log::info;

pub struct PwpProxyTarget {
    name: String,
    vm_id: u32,
    default_url: Option<String>,
    should_proxy: bool,
}

impl PwpProxyTarget {
    pub fn new(settings: PwpTargetSettings) -> Self {
        let name = settings.name.clone();
        let vm_id = settings.vm_id;
        let default_url = settings.default_url.clone();
        let should_proxy = settings.should_proxy;

        if should_proxy {
            if let Some(default_url) = default_url.as_ref() {
                info!(
                    "Creating proxy target '{name}', will proxy to '{default_url}' if URL is not specified in request headers"
                );
            } else {
                info!(
                    "Creating proxy target '{name}', will proxy to URL specified in request headers"
                );
            }

            Self {
                name,
                vm_id,
                default_url,
                should_proxy: true,
            }
        } else {
            info!("Creating proxy target '{name}', will not proxy");

            Self {
                name,
                vm_id,
                default_url: None,
                should_proxy: false,
            }
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn vm_id(&self) -> u32 {
        self.vm_id
    }

    pub fn default_url(&self) -> Option<&str> {
        self.default_url.as_deref()
    }

    pub fn should_proxy(&self) -> bool {
        self.should_proxy
    }
}
