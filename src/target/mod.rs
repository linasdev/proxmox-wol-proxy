use crate::error::PwpError;
use crate::target::settings::PwpTargetSettings;
use axum::http::{HeaderMap, HeaderName, Request, Response, header, response};
use http_body_util::BodyExt;
use log::{info, warn};
use reqwest::{Body, Client, RequestBuilder};
use std::collections::HashSet;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::{Arc, LazyLock};
use url::Url;

pub mod settings;

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

pub struct PwpProxyTarget {
    name: String,
    vm_id: u32,
    default_url: Option<String>,
    should_proxy: bool,
    preserve_host_header: bool,
}

impl PwpProxyTarget {
    pub fn new(settings: PwpTargetSettings) -> Self {
        let name = settings.name.clone();
        let vm_id = settings.vm_id;
        let default_url = settings.default_url.clone();
        let should_proxy = settings.should_proxy;
        let preserve_host_header = settings.preserve_host_header;

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
                preserve_host_header,
            }
        } else {
            info!("Creating proxy target '{name}', will not proxy");

            Self {
                name,
                vm_id,
                default_url: None,
                should_proxy: false,
                preserve_host_header,
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
        proxy_target_client: &Client,
        proxy_url: Url,
        request: Request<axum::body::Body>,
        peer_address: &SocketAddr,
    ) -> Result<Response<axum::body::Body>, PwpError> {
        let mut upstream_request_builder =
            proxy_target_client.request(request.method().clone(), proxy_url.clone());

        upstream_request_builder = self.prepare_headers_for_upstream(
            upstream_request_builder,
            proxy_url,
            request.headers(),
            peer_address,
        );

        upstream_request_builder =
            upstream_request_builder.body(Body::wrap_stream(request.into_data_stream()));

        let upstream_response = upstream_request_builder
            .send()
            .await
            .map_err(Arc::new)
            .map_err(PwpError::ProxyClientError)?;

        let mut downstream_response_builder =
            Response::builder().status(upstream_response.status());

        downstream_response_builder = Self::prepare_headers_for_downstream(
            downstream_response_builder,
            upstream_response.headers(),
        );

        let downstream_response = downstream_response_builder
            .body(axum::body::Body::from_stream(
                upstream_response.bytes_stream(),
            ))
            .map_err(Arc::new)
            .map_err(PwpError::AxumHttpError)?;
        Ok(downstream_response)
    }

    fn prepare_headers_for_upstream(
        &self,
        mut upstream_request_builder: RequestBuilder,
        proxy_url: Url,
        header_map: &HeaderMap,
        peer_address: &SocketAddr,
    ) -> RequestBuilder {
        let dynamic_hop_by_hop_headers = Self::extract_dynamic_hop_by_hop_headers(header_map);

        for (header_name, header_value) in header_map.iter() {
            if Self::should_strip_header(
                header_name,
                &dynamic_hop_by_hop_headers,
                self.preserve_host_header,
            ) {
                continue;
            }

            upstream_request_builder =
                upstream_request_builder.header(header_name.clone(), header_value.clone());
        }

        if !self.preserve_host_header
            && let Some(host) = proxy_url.host()
        {
            let host_header_value = if let Some(port) = proxy_url.port() {
                format!("{}:{}", host, port)
            } else {
                host.to_string()
            };

            upstream_request_builder =
                upstream_request_builder.header(header::HOST, host_header_value)
        }

        if let Some(x_forwarded_for_header_value) =
            header_map.get(HeaderName::from_static("x-forwarded-for"))
        {
            match x_forwarded_for_header_value.to_str() {
                Ok(original_x_forwarded_for) => {
                    let new_x_forwarded_for =
                        format!("{}, {}", original_x_forwarded_for, peer_address.ip());
                    upstream_request_builder.header("x-forwarded-for", new_x_forwarded_for)
                }
                Err(error) => {
                    warn!("Failed to parse 'X-Forwarded-For' header value with error: {error}");
                    upstream_request_builder
                }
            }
        } else {
            upstream_request_builder.header("x-forwarded-for", peer_address.ip().to_string())
        }
    }

    fn prepare_headers_for_downstream(
        mut downstream_response_builder: response::Builder,
        header_map: &HeaderMap,
    ) -> response::Builder {
        let dynamic_hop_by_hop_headers = Self::extract_dynamic_hop_by_hop_headers(header_map);

        for (header_name, header_value) in header_map.iter() {
            if Self::should_strip_header(header_name, &dynamic_hop_by_hop_headers, true) {
                continue;
            }

            downstream_response_builder =
                downstream_response_builder.header(header_name.clone(), header_value.clone());
        }

        downstream_response_builder
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
        preserve_host_header: bool,
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

        if !preserve_host_header && header_name == header::HOST {
            return true;
        }

        false
    }
}
