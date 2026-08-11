use crate::error::PwpError;
use crate::target::settings::PwpTargetSettings;
use actix_web::dev::RequestHead;
use actix_web::http::header::{HeaderMap, HeaderName};
use actix_web::http::{Uri, header};
use actix_web::{HttpResponse, HttpResponseBuilder, web};
use awc::{Client, ClientRequest};
use log::{info, warn};
use std::cell::RefCell;
use std::collections::HashSet;
use std::str::FromStr;
use std::sync::LazyLock;

const HOP_BY_HOP_HEADERS: LazyLock<HashSet<HeaderName>> = LazyLock::new(|| {
    HashSet::from([
        HeaderName::from_static("connection"),
        HeaderName::from_static("keep-alive"),
        HeaderName::from_static("proxy-authenticate"),
        HeaderName::from_static("proxy-authorization"),
        HeaderName::from_static("te"),
        HeaderName::from_static("trailers"),
        HeaderName::from_static("transfer-encoding"),
        HeaderName::from_static("upgrade"),
    ])
});

thread_local! {
    pub(crate) static PROXY_TARGET_CLIENT: RefCell<Option<Client>> = RefCell::new(None);
}

fn with_proxy_target_client<R>(f: impl FnOnce(&Client) -> R) -> R {
    PROXY_TARGET_CLIENT.with_borrow(|client| {
        f(client
            .as_ref()
            .expect("Proxy target client not initialized"))
    })
}

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

    pub async fn proxy(
        &self,
        proxy_uri: Uri,
        payload: web::Payload,
        request_head: &RequestHead,
    ) -> Result<HttpResponse, PwpError> {
        let mut upstream_request =
            with_proxy_target_client(|client| client.request_from(proxy_uri, request_head));
        upstream_request =
            Self::prepare_headers_for_upstream(upstream_request, request_head.headers());

        let upstream_response = match upstream_request.send_stream(payload).await {
            Ok(upstream_response) => upstream_response,
            Err(error) => {
                warn!("Failed to send request to proxy target with error: {error}");
                return Err(PwpError::ProxyError);
            }
        };

        let mut downstream_response_builder = HttpResponse::build(upstream_response.status());
        Self::prepare_headers_for_downstream(
            &mut downstream_response_builder,
            upstream_response.headers(),
        );

        Ok(downstream_response_builder.streaming(upstream_response))
    }

    fn prepare_headers_for_upstream(
        mut upstream_request: ClientRequest,
        header_map: &HeaderMap,
    ) -> ClientRequest {
        let dynamic_hop_by_hop_headers = Self::extract_dynamic_hop_by_hop_headers(header_map);

        for (header_name, header_value) in header_map.iter() {
            if Self::should_strip_header(header_name, &dynamic_hop_by_hop_headers) {
                continue;
            }

            upstream_request =
                upstream_request.insert_header((header_name.clone(), header_value.clone()));
        }

        upstream_request
    }

    fn prepare_headers_for_downstream(
        downstream_response_builder: &mut HttpResponseBuilder,
        header_map: &HeaderMap,
    ) {
        let dynamic_hop_by_hop_headers = Self::extract_dynamic_hop_by_hop_headers(header_map);

        for (header_name, header_value) in header_map.iter() {
            if Self::should_strip_header(header_name, &dynamic_hop_by_hop_headers) {
                continue;
            }

            downstream_response_builder.insert_header((header_name.clone(), header_value.clone()));
        }
    }

    fn extract_dynamic_hop_by_hop_headers(header_map: &HeaderMap) -> HashSet<HeaderName> {
        let mut hop_by_hop_headers = HashSet::new();

        if let Some(connection_header) = header_map.get(header::CONNECTION) {
            if let Ok(connection_header) = connection_header.to_str() {
                for connection_header_part in connection_header.split(',') {
                    let trimmed_connection_header_part = connection_header_part.trim();
                    if !trimmed_connection_header_part.is_empty() {
                        if let Ok(header_name) =
                            HeaderName::from_str(trimmed_connection_header_part)
                        {
                            hop_by_hop_headers.insert(header_name);
                        }
                    }
                }
            }
        }

        hop_by_hop_headers
    }

    fn should_strip_header(
        header_name: &HeaderName,
        dynamic_hop_by_hop_headers: &HashSet<HeaderName>,
    ) -> bool {
        // Strip HTTP/2 and HTTP/3 pseudo-headers
        if header_name.as_str().starts_with(':') {
            return true;
        }

        // Strip standard HTTP/1.1 hop-by-hop headers
        if HOP_BY_HOP_HEADERS.contains(header_name) {
            return true;
        }

        // Strip dynamic hop-by-hop headers specified in the Connection header
        if dynamic_hop_by_hop_headers.contains(header_name) {
            return true;
        }

        false
    }
}
