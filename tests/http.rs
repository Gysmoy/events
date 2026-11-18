// Pruebas de las rutas REST: cada ruta y cada código de estado.

mod common;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use socketioxide::socket::Sid;

use common::*;
use websocket_filter_service::filters::Filters;

fn filters(value: Value) -> Filters {
    match value {
        Value::Object(map) => map,
        _ => panic!("se esperaba un objeto"),
    }
}

#[tokio::test]
async fn health_es_publico() {
    let (router, _) = test_app();
    let reply = call(&router, request(Method::GET, "/health", &[], "")).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.text, r#"{"status":"OK"}"#);
    assert_eq!(reply.headers["content-type"], "application/json; charset=utf-8");
    // Sin CORS en las rutas REST
    let reply = call(&router, request(Method::GET, "/health", &[("origin", ORIGIN)], "")).await;
    assert!(!reply.headers.contains_key("access-control-allow-origin"));
}

#[tokio::test]
async fn ruta_o_metodo_desconocido_es_404() {
    let (router, _) = test_app();
    for (method, uri) in [
        (Method::GET, "/nope"),
        (Method::GET, "/emit"),
        (Method::POST, "/health"),
        (Method::POST, "/stats"),
        (Method::GET, "/clients/filter"),
        (Method::GET, "/socket.io"),
    ] {
        let reply = call(&router, request(method.clone(), uri, &[], "")).await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND, "{method} {uri}");
    }
}

#[tokio::test]
async fn rutas_protegidas_exigen_el_secreto() {
    let (router, _) = test_app();
    let wrong = format!("Bearer {SECRET}x");
    let lower = format!("bearer {SECRET}");
    let without_prefix = SECRET.to_string();
    for (method, uri) in [
        (Method::POST, "/emit"),
        (Method::GET, "/stats"),
        (Method::POST, "/clients/filter"),
    ] {
        for auth in [
            None,
            Some(wrong.as_str()),
            Some(lower.as_str()),
            Some("Bearer "),
            Some(""),
        ] {
            let mut headers = vec![("content-type", "application/json")];
            if let Some(auth) = auth {
                headers.push(("authorization", auth));
            }
            let reply = call(&router, request(method.clone(), uri, &headers, "{}")).await;
            assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{method} {uri} {auth:?}");
            assert_eq!(reply.text, r#"{"error":"No autorizado"}"#);
        }
        // Sin "Bearer " también vale, igual que el replace de JS
        let headers = [
            ("authorization", without_prefix.as_str()),
            ("content-type", "application/json"),
        ];
        let reply = call(&router, request(method.clone(), uri, &headers, "{}")).await;
        assert_ne!(reply.status, StatusCode::UNAUTHORIZED, "{method} {uri}");
    }
}

#[tokio::test]
async fn emit_valida_servicio_filtros_y_evento() {
    let (router, _) = test_app();
    let servicio = r#"{"error":"Se requiere un nombre de servicio válido"}"#;
    let filtros = r#"{"error":"Se requieren filtros válidos (objeto clave-valor)"}"#;
    let evento = r#"{"error":"Se requiere un identificador de evento (eventType)"}"#;
    let cases = [
        (r#"{}"#, servicio),
        (r#"[]"#, servicio),
        (r#"{"service":"","filters":{},"eventType":"x"}"#, servicio),
        (r#"{"service":1,"filters":{},"eventType":"x"}"#, servicio),
        (r#"{"service":["devex"],"filters":{},"eventType":"x"}"#, servicio),
        (r#"{"service":"otro","filters":{},"eventType":"x"}"#, servicio),
        (r#"{"service":"devex","eventType":"x"}"#, filtros),
        (r#"{"service":"devex","filters":null,"eventType":"x"}"#, filtros),
        (r#"{"service":"devex","filters":false,"eventType":"x"}"#, filtros),
        (r#"{"service":"devex","filters":0,"eventType":"x"}"#, filtros),
        (r#"{"service":"devex","filters":"","eventType":"x"}"#, filtros),
        (r#"{"service":"devex","filters":"a=1","eventType":"x"}"#, filtros),
        (r#"{"service":"devex","filters":true,"eventType":"x"}"#, filtros),
        (r#"{"service":"devex","filters":{}}"#, evento),
        (r#"{"service":"devex","filters":{},"eventType":""}"#, evento),
        (r#"{"service":"devex","filters":{},"eventType":5}"#, evento),
        (r#"{"service":"devex","filters":{},"eventType":null}"#, evento),
    ];
    for (body, expected) in cases {
        let reply = post_json(&router, "/emit", body).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(reply.text, expected, "{body}");
    }
}

#[tokio::test]
async fn emit_parsea_el_cuerpo_como_express_json() {
    let (router, _) = test_app();
    let bearer = format!("Bearer {SECRET}");
    let good = r#"{"service":"devex","filters":{},"eventType":"x"}"#;

    // Content-Type que no es JSON: el cuerpo queda {} y falla la validación
    let reply = call(
        &router,
        request(
            Method::POST,
            "/emit",
            &[("authorization", &bearer), ("content-type", "text/plain")],
            good,
        ),
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(reply.json()["error"], "Se requiere un nombre de servicio válido");

    // Sin Content-Type tampoco se parsea
    let reply = call(
        &router,
        request(Method::POST, "/emit", &[("authorization", &bearer)], good),
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);

    // Con parámetros sí
    let reply = call(
        &router,
        request(
            Method::POST,
            "/emit",
            &[
                ("authorization", &bearer),
                ("content-type", "application/json; charset=utf-8"),
            ],
            good,
        ),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);

    // JSON mal formado o que no es objeto/arreglo
    for body in ["{", r#"{"service":}"#, r#""devex""#, "42", "null"] {
        let reply = post_json(&router, "/emit", body).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(reply.text, r#"{"error":"JSON inválido"}"#);
    }

    // Cuerpo vacío -> {}
    let reply = post_json(&router, "/emit", "").await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(reply.json()["error"], "Se requiere un nombre de servicio válido");

    // Más de 100kb
    let big = format!(
        r#"{{"service":"devex","filters":{{}},"eventType":"x","data":"{}"}}"#,
        "x".repeat(110_000)
    );
    let reply = post_json(&router, "/emit", &big).await;
    assert_eq!(reply.status, StatusCode::PAYLOAD_TOO_LARGE);

    // Charset no utf
    let reply = call(
        &router,
        request(
            Method::POST,
            "/emit",
            &[
                ("authorization", &bearer),
                ("content-type", "application/json; charset=latin1"),
            ],
            good,
        ),
    )
    .await;
    assert_eq!(reply.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn emit_sin_clientes_responde_con_los_filtros_completos() {
    let (router, _) = test_app();
    let reply = post_json(
        &router,
        "/emit",
        r#"{"service":"devex","filters":{"environment":"production"},"eventType":"activities_changed"}"#,
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.headers["content-type"], "application/json; charset=utf-8");
    assert_eq!(
        reply.text,
        r#"{"success":true,"message":"Evento enviado","service":"devex","eventType":"activities_changed","clientsNotified":0,"filters":{"service":"devex","environment":"production"}}"#
    );

    // Filtros como arreglo y "service" sobrescrito, igual que el spread de JS
    let reply = post_json(
        &router,
        "/emit",
        r#"{"service":"devex","filters":["a"],"eventType":"x"}"#,
    )
    .await;
    assert_eq!(reply.json()["filters"], json!({"0": "a", "service": "devex"}));
    let reply = post_json(
        &router,
        "/emit",
        r#"{"service":"devex","filters":{"service":"otro"},"eventType":"x"}"#,
    )
    .await;
    assert_eq!(reply.json()["filters"], json!({"service": "otro"}));
    assert_eq!(reply.json()["service"], "devex");

    // Un nombre reservado sin clientes que coincidan no llega a emitirse
    let reply = post_json(
        &router,
        "/emit",
        r#"{"service":"devex","filters":{},"eventType":"connect"}"#,
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
}

#[tokio::test]
async fn emit_cuenta_los_clientes_que_coinciden() {
    let (router, state) = test_app();
    let registry = &state.registry;
    registry.register(
        "devex",
        Sid::new(),
        filters(json!({"service": "devex", "environment": "production", "n": 1})),
    );
    registry.register(
        "devex",
        Sid::new(),
        filters(json!({"service": "devex", "environment": "production", "n": 2})),
    );
    registry.register(
        "devex",
        Sid::new(),
        filters(json!({"service": "devex", "environment": "local"})),
    );

    let count = |body: &'static str| {
        let router = router.clone();
        async move {
            let reply = post_json(&router, "/emit", body).await;
            assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
            reply.json()["clientsNotified"].as_u64().unwrap()
        }
    };
    assert_eq!(count(r#"{"service":"devex","filters":{},"eventType":"x"}"#).await, 3);
    assert_eq!(
        count(r#"{"service":"devex","filters":{"environment":"production"},"eventType":"x"}"#).await,
        2
    );
    assert_eq!(
        count(r#"{"service":"devex","filters":{"n":1.0},"eventType":"x"}"#).await,
        1
    );
    assert_eq!(
        count(r#"{"service":"devex","filters":{"n":"1"},"eventType":"x"}"#).await,
        0
    );
    assert_eq!(
        count(r#"{"service":"devex","filters":{"missing":null},"eventType":"x"}"#).await,
        0
    );

    // Con clientes que coinciden, un nombre reservado falla como en socket.io
    let reply = post_json(
        &router,
        "/emit",
        r#"{"service":"devex","filters":{},"eventType":"disconnect"}"#,
    )
    .await;
    assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn stats_muestra_servicios_y_clientes() {
    let (router, state) = test_app();
    let reply = get_auth(&router, "/stats").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.text, r#"{"totalServices":0,"totalClients":0,"services":{}}"#);

    state.registry.ensure_service("devex");
    let reply = get_auth(&router, "/stats").await;
    assert_eq!(
        reply.text,
        r#"{"totalServices":1,"totalClients":0,"services":{"devex":{"connectedClients":0,"clients":[]}}}"#
    );

    let (a, b) = (Sid::new(), Sid::new());
    state
        .registry
        .register("devex", a, filters(json!({"service": "devex", "user": 1})));
    state
        .registry
        .register("devex", b, filters(json!({"service": "devex", "user": 2})));
    let stats = get_auth(&router, "/stats").await.json();
    assert_eq!(stats["totalServices"], 1);
    assert_eq!(stats["totalClients"], 2);
    let clients = stats["services"]["devex"]["clients"].as_array().unwrap();
    assert_eq!(clients[0]["socketId"], a.to_string());
    assert_eq!(clients[1]["filters"], json!({"service": "devex", "user": 2}));
    let connected_at = clients[0]["connectedAt"].as_str().unwrap();
    assert_eq!(connected_at.len(), 24);
    assert_eq!(&connected_at[10..11], "T");
    assert!(connected_at.ends_with('Z'));
    let keys: Vec<_> = clients[0].as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys, ["socketId", "filters", "connectedAt"]);

    state.registry.remove("devex", a);
    state.registry.remove("devex", b);
    let reply = get_auth(&router, "/stats").await;
    assert_eq!(reply.text, r#"{"totalServices":0,"totalClients":0,"services":{}}"#);
}

#[tokio::test]
async fn clients_filter_lista_sin_emitir() {
    let (router, state) = test_app();
    let a = Sid::new();
    state.registry.register(
        "devex",
        a,
        filters(json!({"service": "devex", "environment": "production"})),
    );
    state.registry.register(
        "devex",
        Sid::new(),
        filters(json!({"service": "devex", "environment": "local"})),
    );

    let reply = post_json(
        &router,
        "/clients/filter",
        r#"{"service":"devex","filters":{"environment":"production"}}"#,
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        reply.text,
        format!(
            r#"{{"service":"devex","matchingClients":1,"clients":[{{"socketId":"{a}","filters":{{"service":"devex","environment":"production"}}}}]}}"#
        )
    );

    // El servicio no se compara con la lista permitida
    let reply = post_json(&router, "/clients/filter", r#"{"service":"cualquiera","filters":{}}"#).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        reply.text,
        r#"{"service":"cualquiera","matchingClients":0,"clients":[]}"#
    );

    for (body, error) in [
        (r#"{"filters":{}}"#, "Se requiere un nombre de servicio válido"),
        (
            r#"{"service":"","filters":{}}"#,
            "Se requiere un nombre de servicio válido",
        ),
        (
            r#"{"service":7,"filters":{}}"#,
            "Se requiere un nombre de servicio válido",
        ),
        (
            r#"{"service":"devex"}"#,
            "Se requieren filtros válidos (objeto clave-valor)",
        ),
        (
            r#"{"service":"devex","filters":"x"}"#,
            "Se requieren filtros válidos (objeto clave-valor)",
        ),
    ] {
        let reply = post_json(&router, "/clients/filter", body).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(reply.json()["error"], error, "{body}");
    }
    let reply = post_json(&router, "/clients/filter", "{").await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
}
