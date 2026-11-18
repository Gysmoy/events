// Utilidades compartidas por las pruebas de integración.
#![allow(dead_code)]

use std::net::SocketAddr;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use websocket_filter_service::auth::{now_seconds, sign};
use websocket_filter_service::config::Config;
use websocket_filter_service::{AppState, app};

pub const SECRET: &str = "test-secret";
pub const ORIGIN: &str = "https://devex.pe";

pub fn test_app() -> (Router, AppState) {
    test_app_with(|_| {})
}

pub fn test_app_with(adjust: impl FnOnce(&mut Config)) -> (Router, AppState) {
    let mut config = Config {
        secret: SECRET.into(),
        allowed_services: vec!["devex".into()],
        allowed_origins: vec![ORIGIN.into(), "https://otro.pe".into()],
        port: 0,
        max_connections_per_ip: 50,
        handshakes_per_minute_per_ip: 60,
    };
    adjust(&mut config);
    app(config)
}

pub struct Reply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub text: String,
}

impl Reply {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.text).unwrap_or_else(|_| panic!("no es JSON: {:?}", self.text))
    }
}

pub async fn call(router: &Router, request: Request<Body>) -> Reply {
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    Reply {
        status,
        headers,
        text: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

// Como un cliente HTTP real: los métodos con cuerpo envían Content-Length
pub fn request(method: Method, uri: &str, headers: &[(&str, &str)], body: &str) -> Request<Body> {
    let sends_body = !matches!(method, Method::GET | Method::HEAD | Method::OPTIONS) || !body.is_empty();
    let mut builder = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    if sends_body {
        builder = builder.header("content-length", body.len());
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

// POST con JSON y el secreto correcto
pub async fn post_json(router: &Router, uri: &str, body: &str) -> Reply {
    let bearer = format!("Bearer {SECRET}");
    call(
        router,
        request(
            Method::POST,
            uri,
            &[("authorization", &bearer), ("content-type", "application/json")],
            body,
        ),
    )
    .await
}

pub async fn get_auth(router: &Router, uri: &str) -> Reply {
    let bearer = format!("Bearer {SECRET}");
    call(router, request(Method::GET, uri, &[("authorization", &bearer)], "")).await
}

pub fn token(service: &str) -> String {
    let expires = (now_seconds() as u64 + 300).to_string();
    format!("{expires}.{}", sign(SECRET, service, &expires))
}

// Agrega el peer TCP a la petición, como into_make_service_with_connect_info
pub fn with_peer(mut request: Request<Body>, peer: Option<SocketAddr>) -> Request<Body> {
    if let Some(peer) = peer {
        request.extensions_mut().insert(ConnectInfo(peer));
    }
    request
}

// Cliente Engine.IO por long-polling, suficiente para probar el protocolo sin red
pub struct Polling {
    router: Router,
    pub sid: String,
    pub handshake: Value,
}

impl Polling {
    pub async fn open(router: &Router) -> Polling {
        Self::open_from(router, None).await
    }

    // Handshake como si nginx lo reenviara con X-Real-IP
    pub async fn open_from(router: &Router, real_ip: Option<&str>) -> Polling {
        Self::open_with_peer(router, real_ip, None).await
    }

    // Handshake con el peer TCP que axum pondría en ConnectInfo
    pub async fn open_with_peer(router: &Router, real_ip: Option<&str>, peer: Option<SocketAddr>) -> Polling {
        let mut headers = vec![("origin", ORIGIN)];
        if let Some(ip) = real_ip {
            headers.push(("x-real-ip", ip));
        }
        let reply = call(
            router,
            with_peer(
                request(Method::GET, "/socket.io/?EIO=4&transport=polling", &headers, ""),
                peer,
            ),
        )
        .await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
        let handshake: Value = serde_json::from_str(reply.text.strip_prefix('0').expect("paquete open")).unwrap();
        Polling {
            router: router.clone(),
            sid: handshake["sid"].as_str().unwrap().to_string(),
            handshake,
        }
    }

    fn uri(&self) -> String {
        format!("/socket.io/?EIO=4&transport=polling&sid={}", self.sid)
    }

    // Envía paquetes Socket.IO (sin el "4" de mensaje Engine.IO)
    pub async fn send(&self, packets: &[&str]) {
        let payload: Vec<String> = packets.iter().map(|packet| format!("4{packet}")).collect();
        let reply = call(
            &self.router,
            request(
                Method::POST,
                &self.uri(),
                &[("origin", ORIGIN)],
                &payload.join("\u{1e}"),
            ),
        )
        .await;
        assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
    }

    // Paquetes Socket.IO recibidos (sin pings ni noops); vacío si no llega nada a tiempo
    pub async fn receive(&self, wait: Duration) -> Vec<String> {
        let call = call(
            &self.router,
            request(Method::GET, &self.uri(), &[("origin", ORIGIN)], ""),
        );
        match tokio::time::timeout(wait, call).await {
            Ok(reply) => reply
                .text
                .split('\u{1e}')
                .filter_map(|packet| packet.strip_prefix('4'))
                .map(String::from)
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    pub async fn receive_one(&self) -> String {
        let packets = self.receive(Duration::from_secs(3)).await;
        assert_eq!(packets.len(), 1, "{packets:?}");
        packets.into_iter().next().unwrap()
    }
}

// "2/devex,[\"evento\",{...}]" -> ["evento", {...}]
pub fn event_args(packet: &str, namespace: &str) -> Value {
    let prefix = format!("2{namespace},");
    serde_json::from_str(
        packet
            .strip_prefix(&prefix)
            .unwrap_or_else(|| panic!("no es un evento: {packet}")),
    )
    .unwrap()
}
