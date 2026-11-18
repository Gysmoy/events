// Pruebas del protocolo Socket.IO por long-polling, sin red.

mod common;

use std::time::Duration;

use axum::http::{Method, StatusCode};
use serde_json::json;

use common::*;

const SHORT: Duration = Duration::from_millis(300);

#[tokio::test]
async fn handshake_exige_origin_permitido() {
    let (router, _) = test_app();
    let uri = "/socket.io/?EIO=4&transport=polling";
    for headers in [
        vec![],
        vec![("origin", "https://evil.pe")],
        vec![("origin", "https://devex.pe/")],
    ] {
        let reply = call(&router, request(Method::GET, uri, &headers, "")).await;
        assert_eq!(reply.status, StatusCode::FORBIDDEN, "{headers:?}");
        assert_eq!(reply.text, r#"{"code":4,"message":"Forbidden"}"#);
        assert!(!reply.headers.contains_key("access-control-allow-origin"));
    }

    // Handshake por websocket directo sin Origin también se rechaza
    let reply = call(
        &router,
        request(Method::GET, "/socket.io/?EIO=4&transport=websocket", &[], ""),
    )
    .await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);

    for origin in [ORIGIN, "https://otro.pe"] {
        let reply = call(&router, request(Method::GET, uri, &[("origin", origin)], "")).await;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(reply.headers["access-control-allow-origin"], origin);
        assert!(reply.text.starts_with("0{"), "{}", reply.text);
    }
}

#[tokio::test]
async fn handshake_con_los_tiempos_de_socket_io() {
    let (router, _) = test_app();
    let polling = Polling::open(&router).await;
    assert_eq!(polling.handshake["upgrades"], json!(["websocket"]));
    assert_eq!(polling.handshake["pingInterval"], 25000);
    assert_eq!(polling.handshake["pingTimeout"], 20000);
    assert_eq!(polling.handshake["maxPayload"], 1_000_000);
}

#[tokio::test]
async fn preflight_cors() {
    let (router, _) = test_app();
    let reply = call(
        &router,
        request(
            Method::OPTIONS,
            "/socket.io/?EIO=4&transport=polling",
            &[
                ("origin", ORIGIN),
                ("access-control-request-method", "POST"),
                ("access-control-request-headers", "content-type"),
            ],
            "",
        ),
    )
    .await;
    assert!(reply.status.is_success());
    assert_eq!(reply.headers["access-control-allow-origin"], ORIGIN);
    assert_eq!(reply.headers["access-control-allow-methods"], "GET,POST");
    assert_eq!(reply.headers["access-control-allow-headers"], "content-type");
}

#[tokio::test]
async fn sesion_abierta_no_vuelve_a_pedir_origin() {
    let (router, _) = test_app();
    let polling = Polling::open(&router).await;
    let uri = format!("/socket.io/?EIO=4&transport=polling&sid={}", polling.sid);
    let reply = call(&router, request(Method::POST, &uri, &[], "40/devex,{}")).await;
    assert_eq!(reply.status, StatusCode::OK);
}

#[tokio::test]
async fn namespace_raiz_e_invalido() {
    let (router, _) = test_app();
    let polling = Polling::open(&router).await;
    polling.send(&["0"]).await;
    assert_eq!(polling.receive_one().await, r#"4{"message":"No autorizado"}"#);

    let polling = Polling::open(&router).await;
    polling.send(&["0/x,"]).await;
    assert_eq!(polling.receive_one().await, r#"4/x,{"message":"Invalid namespace"}"#);
}

#[tokio::test]
async fn autenticacion_del_namespace() {
    let (router, _) = test_app();
    let expired = {
        let sig = websocket_filter_service::auth::sign(SECRET, "devex", "1000");
        format!("1000.{sig}")
    };
    let other = token("otro");
    // Firma válida con el último dígito hex cambiado
    let bad = {
        let mut good = token("devex");
        let last = good.pop().unwrap();
        good.push(if last == '0' { '1' } else { '0' });
        good
    };
    let rejected = [
        "0/devex,".to_string(),
        "0/devex,{}".to_string(),
        r#"0/devex,{"token":null}"#.to_string(),
        r#"0/devex,{"token":12345}"#.to_string(),
        format!(r#"0/devex,{{"token":["{}"]}}"#, token("devex")),
        format!(r#"0/devex,{{"token":"{expired}"}}"#),
        format!(r#"0/devex,{{"token":"{other}"}}"#),
        format!(r#"0/devex,{{"token":"{bad}"}}"#),
    ];
    for connect in &rejected {
        let polling = Polling::open(&router).await;
        polling.send(&[connect]).await;
        assert_eq!(
            polling.receive_one().await,
            r#"4/devex,{"message":"No autorizado"}"#,
            "{connect}"
        );
    }

    let polling = Polling::open(&router).await;
    polling
        .send(&[&format!(r#"0/devex,{{"token":"{}"}}"#, token("devex"))])
        .await;
    let connected = polling.receive_one().await;
    assert!(connected.starts_with(r#"0/devex,{"sid":""#), "{connected}");
}

async fn connect(router: &axum::Router) -> Polling {
    let polling = Polling::open(router).await;
    polling
        .send(&[&format!(r#"0/devex,{{"token":"{}"}}"#, token("devex"))])
        .await;
    assert!(polling.receive_one().await.starts_with("0/devex,"));
    polling
}

#[tokio::test]
async fn registrar_actualizar_y_consultar_filtros() {
    let (router, state) = test_app();
    let polling = connect(&router).await;

    // El servicio existe apenas se conecta, aunque aún no registre filtros
    let stats = get_auth(&router, "/stats").await.json();
    assert_eq!(stats["services"]["devex"]["connectedClients"], 0);

    // update_filters sin registrar no responde nada; get_filters responde error
    polling
        .send(&[r#"2/devex,["update_filters",{"a":1}]"#, r#"2/devex,["get_filters"]"#])
        .await;
    let packets = polling.receive(Duration::from_secs(3)).await;
    assert_eq!(packets.len(), 1, "{packets:?}");
    assert_eq!(
        event_args(&packets[0], "/devex"),
        json!(["error", {"message": "No hay filtros registrados para este cliente"}])
    );

    for invalid in [r#"[]"#, r#"[null]"#, r#"["texto"]"#, r#"[5]"#] {
        let args = invalid.trim_start_matches('[').trim_end_matches(']');
        let packet = if args.is_empty() {
            r#"2/devex,["register_filters"]"#.to_string()
        } else {
            format!(r#"2/devex,["register_filters",{args}]"#)
        };
        polling.send(&[&packet]).await;
        assert_eq!(
            event_args(&polling.receive_one().await, "/devex"),
            json!(["error", {"message": "Los filtros deben ser un objeto válido"}]),
            "{packet}"
        );
    }

    polling
        .send(&[r#"2/devex,["register_filters",{"environment":"production","service":"hack"}]"#])
        .await;
    assert_eq!(
        event_args(&polling.receive_one().await, "/devex"),
        json!(["filters_registered", {
            "message": "Filtros registrados correctamente",
            "service": "devex",
            "filters": {"service": "hack", "environment": "production"}
        }])
    );

    polling.send(&[r#"2/devex,["update_filters",["x"]]"#]).await;
    assert_eq!(
        event_args(&polling.receive_one().await, "/devex"),
        json!(["filters_updated", {
            "message": "Filtros actualizados correctamente",
            "service": "devex",
            "filters": {"0": "x", "service": "devex"}
        }])
    );

    polling.send(&[r#"2/devex,["update_filters","x"]"#]).await;
    assert_eq!(
        event_args(&polling.receive_one().await, "/devex"),
        json!(["error", {"message": "Los filtros deben ser un objeto válido"}])
    );

    polling.send(&[r#"2/devex,["get_filters"]"#]).await;
    assert_eq!(
        event_args(&polling.receive_one().await, "/devex"),
        json!(["current_filters", {"service": "devex", "filters": {"0": "x", "service": "devex"}}])
    );

    assert_eq!(state.registry.stats()["totalClients"], 1);
}

#[tokio::test]
async fn emit_llega_solo_a_los_que_coinciden_con_un_solo_argumento() {
    let (router, _) = test_app();
    let production = connect(&router).await;
    production
        .send(&[r#"2/devex,["register_filters",{"environment":"production","n":1,"obj":{"a":1}}]"#])
        .await;
    production.receive_one().await;
    let local = connect(&router).await;
    local
        .send(&[r#"2/devex,["register_filters",{"environment":"local"}]"#])
        .await;
    local.receive_one().await;

    let cases = [
        (r#""data":[1,2,3]"#, json!([1, 2, 3])),
        (r#""data":{"id":9}"#, json!({"id": 9})),
        (r#""data":"hola""#, json!("hola")),
        (r#""data":42"#, json!(42)),
        (r#""data":true"#, json!(true)),
        ("\"data\":null", json!({})),
        ("\"data\":0", json!({})),
        ("\"data\":\"\"", json!({})),
        ("\"data\":false", json!({})),
        ("\"x\":1", json!({})),
    ];
    for (data, expected) in cases {
        let body = format!(
            r#"{{"service":"devex","filters":{{"environment":"production","n":1.0}},"eventType":"cambio",{data}}}"#
        );
        let reply = post_json(&router, "/emit", &body).await;
        assert_eq!(reply.json()["clientsNotified"], 1, "{data}");
        assert_eq!(
            event_args(&production.receive_one().await, "/devex"),
            json!(["cambio", expected]),
            "{data}"
        );
        assert!(local.receive(SHORT).await.is_empty(), "{data}");
    }

    // Un objeto como filtro nunca coincide
    let reply = post_json(
        &router,
        "/emit",
        r#"{"service":"devex","filters":{"obj":{"a":1}},"eventType":"cambio"}"#,
    )
    .await;
    assert_eq!(reply.json()["clientsNotified"], 0);
    assert!(production.receive(SHORT).await.is_empty());
}

#[tokio::test]
async fn desconectar_borra_al_cliente_y_al_servicio_vacio() {
    let (router, state) = test_app();
    let polling = connect(&router).await;
    polling.send(&[r#"2/devex,["register_filters",{"a":1}]"#]).await;
    polling.receive_one().await;
    assert_eq!(state.registry.stats()["totalClients"], 1);

    polling.send(&["1/devex,"]).await;
    for _ in 0..50 {
        if state.registry.stats()["totalServices"] == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        state.registry.stats(),
        json!({"totalServices": 0, "totalClients": 0, "services": {}})
    );
}
