// Pruebas de los topes por IP: handshakes por minuto y conexiones abiertas a la vez.

mod common;

use std::net::SocketAddr;
use std::time::Duration;

use axum::Router;
use axum::http::{Method, StatusCode};

use common::*;

const HANDSHAKE: &str = "/socket.io/?EIO=4&transport=polling";

async fn handshake(router: &Router, origin: &str, real_ip: Option<&str>) -> Reply {
    let mut headers = vec![("origin", origin)];
    if let Some(ip) = real_ip {
        headers.push(("x-real-ip", ip));
    }
    call(router, request(Method::GET, HANDSHAKE, &headers, "")).await
}

// Abre sesión desde una IP y entra a /devex con un token válido; devuelve la respuesta
async fn connect_from(router: &Router, real_ip: Option<&str>) -> (Polling, String) {
    let polling = Polling::open_from(router, real_ip).await;
    polling
        .send(&[&format!(r#"0/devex,{{"token":"{}"}}"#, token("devex"))])
        .await;
    let packet = polling.receive_one().await;
    (polling, packet)
}

fn connected(packet: &str) -> bool {
    packet.starts_with(r#"0/devex,{"sid":""#)
}

const TOO_MANY: &str = r#"4/devex,{"message":"Demasiadas conexiones"}"#;

#[tokio::test]
async fn un_peer_publico_no_puede_falsificar_su_ip() {
    let (router, _) = test_app_with(|config| {
        config.handshakes_per_minute_per_ip = 2;
        config.max_connections_per_ip = 1;
    });
    let public: SocketAddr = "200.7.7.7:50000".parse().unwrap();
    // Cada handshake inventa otra X-Real-IP, pero cuenta para el peer real
    let mut statuses = Vec::new();
    for fake in ["1.1.1.1", "2.2.2.2", "3.3.3.3"] {
        let request = request(Method::GET, HANDSHAKE, &[("origin", ORIGIN), ("x-real-ip", fake)], "");
        statuses.push(call(&router, with_peer(request, Some(public))).await.status);
    }
    assert_eq!(
        statuses,
        [StatusCode::OK, StatusCode::OK, StatusCode::TOO_MANY_REQUESTS]
    );

    // El tope de conexiones también lee el peer guardado en la sesión
    let (router, _) = test_app_with(|config| config.max_connections_per_ip = 1);
    let connect = |fake: &'static str| {
        let router = router.clone();
        async move {
            let polling = Polling::open_with_peer(&router, Some(fake), Some(public)).await;
            polling
                .send(&[&format!(r#"0/devex,{{"token":"{}"}}"#, token("devex"))])
                .await;
            (polling.receive_one().await, polling)
        }
    };
    let (first, _a) = connect("1.1.1.1").await;
    assert!(connected(&first), "{first}");
    let (second, _b) = connect("2.2.2.2").await;
    assert_eq!(second, TOO_MANY);
}

#[tokio::test]
async fn handshakes_por_ip_tienen_tope() {
    let (router, _) = test_app_with(|config| config.handshakes_per_minute_per_ip = 3);
    for _ in 0..3 {
        assert_eq!(
            handshake(&router, ORIGIN, Some("200.1.2.3")).await.status,
            StatusCode::OK
        );
    }
    let reply = handshake(&router, ORIGIN, Some("200.1.2.3")).await;
    assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(reply.text, r#"{"code":4,"message":"Demasiadas conexiones"}"#);

    // Otra IP sigue entrando
    assert_eq!(
        handshake(&router, ORIGIN, Some("200.1.2.4")).await.status,
        StatusCode::OK
    );
    // Sin IP real conocida no se limita
    for _ in 0..10 {
        assert_eq!(handshake(&router, ORIGIN, None).await.status, StatusCode::OK);
    }
}

#[tokio::test]
async fn origin_rechazado_no_gasta_handshakes() {
    let (router, _) = test_app_with(|config| config.handshakes_per_minute_per_ip = 1);
    for _ in 0..5 {
        let reply = handshake(&router, "https://evil.pe", Some("200.1.2.3")).await;
        assert_eq!(reply.status, StatusCode::FORBIDDEN);
    }
    assert_eq!(
        handshake(&router, ORIGIN, Some("200.1.2.3")).await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn una_sesion_abierta_no_gasta_handshakes() {
    let (router, _) = test_app_with(|config| config.handshakes_per_minute_per_ip = 1);
    // El único handshake de la IP alcanza para entrar al namespace y seguir usando la sesión
    let (polling, packet) = connect_from(&router, Some("200.1.2.3")).await;
    assert!(connected(&packet), "{packet}");
    polling.send(&[r#"2/devex,["get_filters"]"#]).await;
    assert!(polling.receive_one().await.contains("No hay filtros registrados"));
    // Un handshake nuevo de esa IP ya no
    assert_eq!(
        handshake(&router, ORIGIN, Some("200.1.2.3")).await.status,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn conexiones_por_ip_tienen_tope() {
    let (router, state) = test_app_with(|config| config.max_connections_per_ip = 2);
    let (first, packet) = connect_from(&router, Some("200.1.2.3")).await;
    assert!(connected(&packet), "{packet}");
    let (_second, packet) = connect_from(&router, Some("200.1.2.3")).await;
    assert!(connected(&packet), "{packet}");
    let (_third, packet) = connect_from(&router, Some("200.1.2.3")).await;
    assert_eq!(packet, TOO_MANY);

    // Otra IP tiene su propio cupo; la misma /64 de IPv6 cuenta como una sola
    let (_other, packet) = connect_from(&router, Some("200.1.2.4")).await;
    assert!(connected(&packet), "{packet}");
    let (_v6a, packet) = connect_from(&router, Some("2800:200:1:2::1")).await;
    assert!(connected(&packet), "{packet}");
    let (_v6b, packet) = connect_from(&router, Some("2800:200:1:2::2")).await;
    assert!(connected(&packet), "{packet}");
    let (_v6c, packet) = connect_from(&router, Some("2800:200:1:2::3")).await;
    assert_eq!(packet, TOO_MANY);

    // Al cerrar una conexión, la IP vuelve a tener lugar
    first.send(&["1/devex,"]).await;
    let open = || state.io.of("/devex").unwrap().sockets().len();
    for _ in 0..50 {
        if open() == 4 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(open(), 4);
    let (_again, packet) = connect_from(&router, Some("200.1.2.3")).await;
    assert!(connected(&packet), "{packet}");
}

#[tokio::test]
async fn token_invalido_se_rechaza_antes_que_el_tope() {
    let (router, _) = test_app_with(|config| config.max_connections_per_ip = 1);
    let (_first, packet) = connect_from(&router, Some("200.1.2.3")).await;
    assert!(connected(&packet), "{packet}");
    let polling = Polling::open_from(&router, Some("200.1.2.3")).await;
    polling.send(&[r#"0/devex,{"token":"1.x"}"#]).await;
    assert_eq!(polling.receive_one().await, r#"4/devex,{"message":"No autorizado"}"#);
}

#[tokio::test]
async fn sin_ip_conocida_o_con_tope_cero_no_se_limita() {
    let (router, _) = test_app_with(|config| config.max_connections_per_ip = 1);
    let mut open = Vec::new();
    for _ in 0..3 {
        let (polling, packet) = connect_from(&router, None).await;
        assert!(connected(&packet), "{packet}");
        open.push(polling);
    }

    let (router, _) = test_app_with(|config| {
        config.max_connections_per_ip = 0;
        config.handshakes_per_minute_per_ip = 0;
    });
    for _ in 0..3 {
        let (polling, packet) = connect_from(&router, Some("200.1.2.3")).await;
        assert!(connected(&packet), "{packet}");
        open.push(polling);
    }
}
