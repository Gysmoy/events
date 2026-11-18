// Endpoints REST para los backends y la entrada de Socket.IO.

use std::net::{IpAddr, SocketAddr};

use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Extensions, HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};
use socketioxide::layer::SocketIoLayer;
use tower::ServiceBuilder;
use tower_http::cors::{AllowHeaders, AllowOrigin, CorsLayer};

use crate::AppState;
use crate::auth::{bearer_token, safe_equal};
use crate::body::{BodyError, read_json};
use crate::config::Config;
use crate::filters::{complete_filters, is_js_falsy, is_js_object};
use crate::limits::client_ip;
use crate::sockets::{self, NO_AUTORIZADO};

const SERVICIO_INVALIDO: &str = "Se requiere un nombre de servicio válido";
const FILTROS_INVALIDOS: &str = "Se requieren filtros válidos (objeto clave-valor)";
const EVENTO_INVALIDO: &str = "Se requiere un identificador de evento (eventType)";

pub fn router(state: AppState, socket_layer: SocketIoLayer) -> Router {
    // CORS y control de Origin solo para Socket.IO; las rutas REST no llevan CORS
    let socket_io = ServiceBuilder::new()
        .layer(cors(&state.config))
        .layer(middleware::from_fn_with_state(state.clone(), guard_handshake))
        .layer(socket_layer)
        .service(Router::<()>::new());

    Router::new()
        .route("/emit", post(emit))
        .route("/stats", get(stats))
        .route("/clients/filter", post(clients_filter))
        .route("/health", get(health))
        .route_service("/socket.io/", socket_io)
        // Como Express: ruta desconocida o método equivocado -> 404
        .fallback(not_found)
        .method_not_allowed_fallback(not_found)
        .with_state(state)
}

// Igual que la opción cors de socket.io: refleja el origen permitido, métodos GET y POST
fn cors(config: &Config) -> CorsLayer {
    // "*" no es comodín en la lista de socket.io: ningún navegador envía "Origin: *"
    let origins: Vec<HeaderValue> = config
        .allowed_origins
        .iter()
        .filter(|origin| origin.as_str() != "*")
        .filter_map(|origin| HeaderValue::from_str(origin).ok())
        .collect();
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([Method::GET, Method::POST])
        .allow_headers(AllowHeaders::mirror_request())
}

// allowRequest: el handshake de Engine.IO (sin sid) exige un Origin permitido y
// respeta el tope de handshakes por IP. Las peticiones de una sesión ya abierta
// (polling o el upgrade a websocket) pasan.
async fn guard_handshake(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let is_handshake = !request
        .uri()
        .query()
        .unwrap_or_default()
        .split('&')
        .any(|pair| pair.strip_prefix("sid=").is_some_and(|sid| !sid.is_empty()));

    if is_handshake {
        let mut origins = request.headers().get_all(header::ORIGIN).iter();
        let allowed = match (origins.next(), origins.next()) {
            (Some(origin), None) => origin.to_str().is_ok_and(|origin| state.config.allows_origin(origin)),
            _ => false,
        };
        if !allowed {
            // Misma respuesta que Engine.IO para allowRequest rechazado
            return json_response(StatusCode::FORBIDDEN, &json!({ "code": 4, "message": "Forbidden" }));
        }

        let ip = client_ip(request.headers(), peer_ip(request.extensions()));
        if ip.is_some_and(|ip| !state.handshakes.allow(ip)) {
            return json_response(
                StatusCode::TOO_MANY_REQUESTS,
                &json!({ "code": 4, "message": "Demasiadas conexiones" }),
            );
        }
    }
    next.run(request).await
}

// IP del peer TCP que axum deja en las extensiones (into_make_service_with_connect_info)
pub fn peer_ip(extensions: &Extensions) -> Option<IpAddr> {
    extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip())
}

// Por qué se rechaza una petición antes de llegar a las validaciones
enum Rejection {
    Unauthorized,
    Body(BodyError),
}

impl IntoResponse for Rejection {
    fn into_response(self) -> Response {
        match self {
            Rejection::Unauthorized => json_response(StatusCode::UNAUTHORIZED, &json!({ "error": NO_AUTORIZADO })),
            Rejection::Body(error) => json_response(error.status(), &error.body()),
        }
    }
}

// Emitir y consultar es solo para los backends: requiere el secreto
fn require_secret(config: &Config, headers: &HeaderMap) -> Result<(), Rejection> {
    let token = bearer_token(headers.get(header::AUTHORIZATION).map(HeaderValue::as_bytes));
    if safe_equal(token, config.secret.as_bytes()) {
        Ok(())
    } else {
        Err(Rejection::Unauthorized)
    }
}

// Lee el cuerpo como express.json() después de validar el secreto
async fn authorized_body(config: &Config, headers: &HeaderMap, body: Body) -> Result<Value, Rejection> {
    require_secret(config, headers)?;
    read_json(headers, body).await.map_err(Rejection::Body)
}

fn non_empty_str(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str).filter(|s| !s.is_empty())
}

// Endpoint REST para enviar eventos
async fn emit(State(state): State<AppState>, headers: HeaderMap, body: Body) -> Response {
    let body = match authorized_body(&state.config, &headers, body).await {
        Ok(body) => body,
        Err(rejection) => return rejection.into_response(),
    };

    // Validaciones
    let Some(service) = non_empty_str(body.get("service")).filter(|s| state.config.allows_service(s)) else {
        return bad_request(SERVICIO_INVALIDO);
    };
    let Some(filters) = body.get("filters").filter(|f| is_js_object(f)) else {
        return bad_request(FILTROS_INVALIDOS);
    };
    let Some(event_type) = non_empty_str(body.get("eventType")) else {
        return bad_request(EVENTO_INVALIDO);
    };

    // Los filtros deben incluir el servicio
    let complete = complete_filters(service, filters);

    // Clientes que coinciden en el servicio; el candado se suelta antes de emitir
    let sids = state.registry.matching_ids(service, &complete);

    if !sids.is_empty() {
        // socket.io lanza un error con un nombre reservado y Express responde 500
        if sockets::is_reserved_event(event_type) {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &json!({ "error": format!("\"{event_type}\" es un nombre de evento reservado") }),
            );
        }
        // Enviar solo eventType y data (o {} si data es falsy)
        let data = body.get("data");
        let payload = if is_js_falsy(data) {
            json!({})
        } else {
            data.cloned().unwrap_or_default()
        };
        sockets::emit_to(&state.io, service, &sids, event_type, &payload).await;
    }

    json_response(
        StatusCode::OK,
        &json!({
            "success": true,
            "message": "Evento enviado",
            "service": service,
            "eventType": event_type,
            "clientsNotified": sids.len(),
            "filters": complete,
        }),
    )
}

// Endpoint para obtener estadísticas
async fn stats(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(rejection) = require_secret(&state.config, &headers) {
        return rejection.into_response();
    }
    json_response(StatusCode::OK, &state.registry.stats())
}

// Endpoint para obtener clientes por filtros específicos en un servicio
async fn clients_filter(State(state): State<AppState>, headers: HeaderMap, body: Body) -> Response {
    let body = match authorized_body(&state.config, &headers, body).await {
        Ok(body) => body,
        Err(rejection) => return rejection.into_response(),
    };

    let Some(service) = non_empty_str(body.get("service")) else {
        return bad_request(SERVICIO_INVALIDO);
    };
    let Some(filters) = body.get("filters").filter(|f| is_js_object(f)) else {
        return bad_request(FILTROS_INVALIDOS);
    };

    let complete = complete_filters(service, filters);
    let clients = state.registry.matching_clients(service, &complete);

    json_response(
        StatusCode::OK,
        &json!({
            "service": service,
            "matchingClients": clients.len(),
            "clients": clients,
        }),
    )
}

// Endpoint de salud
async fn health() -> Response {
    json_response(StatusCode::OK, &json!({ "status": "OK" }))
}

async fn not_found() -> StatusCode {
    StatusCode::NOT_FOUND
}

fn bad_request(message: &str) -> Response {
    json_response(StatusCode::BAD_REQUEST, &json!({ "error": message }))
}

// Igual que res.json de Express
fn json_response(status: StatusCode, body: &Value) -> Response {
    let bytes = serde_json::to_vec(body).unwrap_or_default();
    (
        status,
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json; charset=utf-8"),
        )],
        bytes,
    )
        .into_response()
}
