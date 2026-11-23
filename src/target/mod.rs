use crate::error::PwpError;
use crate::target::settings::PwpTargetSettings;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Request, Response, header, response};
use http_body_util::BodyExt;
use log::{info, warn};
use reqwest::{Body, Client, RequestBuilder};
use std::collections::HashSet;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::{Arc, LazyLock};
use std::time::Duration;
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
    max_idle_duration: Option<Duration>,
    should_proxy: bool,
    preserve_host_header: bool,
}

impl PwpProxyTarget {
    pub fn new(settings: PwpTargetSettings) -> Self {
        let name = settings.name.clone();
        let vm_id = settings.vm_id;
        let default_url = settings.default_url.clone();
        let max_idle_duration = settings.max_idle_secs.map(Duration::from_secs);
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
                max_idle_duration,
                should_proxy: true,
                preserve_host_header,
            }
        } else {
            info!("Creating proxy target '{name}', will not proxy");

            Self {
                name,
                vm_id,
                default_url: None,
                max_idle_duration,
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

    pub fn max_idle_duration(&self) -> Option<Duration> {
        self.max_idle_duration
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

            if header_name == "x-forwarded-for" {
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

        let mut x_forwarded_for = header_map
            .get_all("x-forwarded-for")
            .iter()
            .map(HeaderValue::as_bytes)
            .collect::<Vec<_>>()
            .join(&b", "[..]);

        if !x_forwarded_for.is_empty() {
            x_forwarded_for.extend_from_slice(b", ");
        }
        x_forwarded_for.extend_from_slice(peer_address.ip().to_string().as_bytes());

        match HeaderValue::from_bytes(&x_forwarded_for) {
            Ok(x_forwarded_for) => {
                upstream_request_builder.header("x-forwarded-for", x_forwarded_for)
            }
            Err(error) => {
                warn!("Failed to build 'X-Forwarded-For' header value with error: {error}");
                upstream_request_builder
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use googletest::prelude::*;

    fn settings() -> PwpTargetSettings {
        PwpTargetSettings {
            name: "vm1".to_string(),
            vm_id: 101,
            default_url: Some("https://vm1".to_string()),
            max_idle_secs: Some(60),
            should_proxy: true,
            preserve_host_header: false,
        }
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(
                HeaderName::from_str(name).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    #[gtest]
    fn new_exposes_settings() {
        let target = PwpProxyTarget::new(settings());

        assert_that!(target.name(), eq("vm1"));
        assert_that!(target.vm_id(), eq(101));
        assert_that!(target.default_url(), eq(Some("https://vm1")));
        assert_that!(
            target.max_idle_duration(),
            eq(Some(Duration::from_secs(60)))
        );
        assert_that!(target.should_proxy(), is_true());
    }

    #[gtest]
    fn new_without_proxying_drops_default_url() {
        let target = PwpProxyTarget::new(PwpTargetSettings {
            should_proxy: false,
            ..settings()
        });

        assert_that!(target.should_proxy(), is_false());
        assert_that!(target.default_url(), eq(None));
    }

    #[gtest]
    fn dynamic_hop_by_hop_headers_come_from_connection_header() {
        let map = headers(&[("connection", "X-Foo, , x-bar,not a header!")]);
        let result = PwpProxyTarget::extract_dynamic_hop_by_hop_headers(&map);

        assert_that!(
            result,
            unordered_elements_are![
                eq(&HeaderName::from_static("x-foo")),
                eq(&HeaderName::from_static("x-bar")),
            ]
        );
    }

    #[gtest]
    fn dynamic_hop_by_hop_headers_empty_without_connection_header() {
        let result = PwpProxyTarget::extract_dynamic_hop_by_hop_headers(&HeaderMap::new());

        assert_that!(result, is_empty());
    }

    #[gtest]
    fn should_strip_header_rules() {
        let none = HashSet::new();
        let dynamic = HashSet::from([HeaderName::from_static("x-dynamic")]);

        assert_that!(
            PwpProxyTarget::should_strip_header(&header::UPGRADE, &none, true),
            is_true()
        );
        assert_that!(
            PwpProxyTarget::should_strip_header(
                &HeaderName::from_static("x-dynamic"),
                &dynamic,
                true
            ),
            is_true()
        );
        assert_that!(
            PwpProxyTarget::should_strip_header(&HeaderName::from_static("x-dynamic"), &none, true),
            is_false()
        );
        assert_that!(
            PwpProxyTarget::should_strip_header(&header::HOST, &none, false),
            is_true()
        );
        assert_that!(
            PwpProxyTarget::should_strip_header(&header::HOST, &none, true),
            is_false()
        );
        assert_that!(
            PwpProxyTarget::should_strip_header(&header::ACCEPT, &none, false),
            is_false()
        );
    }

    #[gtest]
    fn downstream_headers_strip_hop_by_hop_but_keep_host() {
        let upstream = headers(&[
            ("connection", "x-secret"),
            ("x-secret", "1"),
            ("upgrade", "websocket"),
            ("host", "example"),
            ("content-type", "text/plain"),
        ]);
        let response =
            PwpProxyTarget::prepare_headers_for_downstream(Response::builder(), &upstream)
                .body(())
                .unwrap();
        let result = response.headers();

        assert_that!(result.get("connection"), none());
        assert_that!(result.get("x-secret"), none());
        assert_that!(result.get("upgrade"), none());
        assert_that!(result.get("host").unwrap(), eq("example"));
        assert_that!(result.get("content-type").unwrap(), eq("text/plain"));
    }

    fn build_upstream(target: &PwpProxyTarget, url: &str, map: &HeaderMap) -> reqwest::Request {
        let client = Client::new();
        target
            .prepare_headers_for_upstream(
                client.get(url),
                Url::parse(url).unwrap(),
                map,
                &"10.0.0.5:1234".parse().unwrap(),
            )
            .build()
            .unwrap()
    }

    #[gtest]
    fn upstream_headers_rewrite_host_and_set_forwarded_for() {
        let target = PwpProxyTarget::new(settings());
        let request = build_upstream(
            &target,
            "https://vm1.example:8443/path",
            &headers(&[("host", "original"), ("accept", "*/*")]),
        );

        assert_that!(
            request.headers().get("host").unwrap(),
            eq("vm1.example:8443")
        );
        assert_that!(request.headers().get("accept").unwrap(), eq("*/*"));
        assert_that!(
            request.headers().get("x-forwarded-for").unwrap(),
            eq("10.0.0.5")
        );
    }

    #[gtest]
    fn upstream_headers_append_to_existing_forwarded_for() {
        let target = PwpProxyTarget::new(settings());
        let request = build_upstream(
            &target,
            "https://vm1.example/",
            &headers(&[("x-forwarded-for", "1.2.3.4")]),
        );

        assert_that!(
            request.headers().get_all("x-forwarded-for").iter().count(),
            eq(1)
        );
        assert_that!(
            request.headers().get("x-forwarded-for").unwrap(),
            eq("1.2.3.4, 10.0.0.5")
        );
    }

    #[gtest]
    fn upstream_headers_preserve_host_when_configured() {
        let target = PwpProxyTarget::new(PwpTargetSettings {
            preserve_host_header: true,
            ..settings()
        });
        let request = build_upstream(
            &target,
            "https://vm1.example/",
            &headers(&[("host", "original")]),
        );

        assert_that!(request.headers().get("host").unwrap(), eq("original"));
    }

    #[gtest]
    fn upstream_headers_append_peer_to_non_utf8_forwarded_for() {
        let target = PwpProxyTarget::new(settings());
        let mut map = HeaderMap::new();
        map.insert(
            "x-forwarded-for",
            header::HeaderValue::from_bytes(b"1.2.3.\xff").unwrap(),
        );

        let request = build_upstream(&target, "https://vm1.example/", &map);

        assert_that!(
            request.headers().get("x-forwarded-for").unwrap().as_bytes(),
            eq(b"1.2.3.\xff, 10.0.0.5")
        );
    }

    #[gtest]
    fn upstream_headers_merge_multiple_forwarded_for_headers() {
        let target = PwpProxyTarget::new(settings());
        let request = build_upstream(
            &target,
            "https://vm1.example/",
            &headers(&[
                ("x-forwarded-for", "1.1.1.1"),
                ("x-forwarded-for", "2.2.2.2"),
            ]),
        );

        assert_that!(
            request.headers().get_all("x-forwarded-for").iter().count(),
            eq(1)
        );
        assert_that!(
            request.headers().get("x-forwarded-for").unwrap(),
            eq("1.1.1.1, 2.2.2.2, 10.0.0.5")
        );
    }
}
