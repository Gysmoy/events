// Capa Socket.IO: un namespace por servicio permitido, autenticado con token firmado.

use std::future::{Ready, ready};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use socketioxide::SocketIo;
use socketioxide::extract::{SocketRef, TryData};
use socketioxide::handler::ConnectHandler;
use socketioxide::layer::SocketIoLayer;
use socketioxide::socket::Sid;

use crate::auth::{is_valid_socket_token, now_seconds};
use crate::config::Config;
use crate::filters::{complete_filters, is_js_object};
use crate::http::peer_ip;
use crate::limits::{client_ip, limit_key};
use crate::state::Registry;

pub const NO_AUTORIZADO: &str = "No autorizado";
pub const DEMASIADAS_CONEXIONES: &str = "Demasiadas conexiones";
const FILTROS_INVALIDOS: &str = "Los filtros deben ser un objeto válido";
const SIN_FILTROS: &str = "No hay filtros registrados para este cliente";

// Mismos valores por defecto que socket.io 4
const PING_INTERVAL: Duration = Duration::from_secs(25);
const PING_TIMEOUT: Duration = Duration::from_secs(20);
// Tiempo que una sesión puede estar abierta sin entrar a un namespace (socket.io usa 45 s).
// El panel en frío tarda ~6 s entre el handshake y pedir su token: 15 s le dan margen y
// una sesión sin token se cierra 3 veces antes. Si se pasa, el cliente reconecta solo.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
// maxHttpBufferSize: 1e6 bytes, también para mensajes por websocket
pub const MAX_HTTP_BUFFER_SIZE: usize = 1_000_000;

// Eventos que socket.io no deja emitir desde el servidor
const RESERVED_EVENTS: [&str; 6] = [
    "connect",
    "connect_error",
    "disconnect",
    "disconnecting",
    "newListener",
    "removeListener",
];

pub fn is_reserved_event(event: &str) -> bool {
    RESERVED_EVENTS.contains(&event)
}

pub fn layer(config: &Arc<Config>, registry: &Arc<Registry>) -> (SocketIoLayer, SocketIo) {
    let (layer, io) = SocketIo::builder()
        .ping_interval(PING_INTERVAL)
        .ping_timeout(PING_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .max_payload(MAX_HTTP_BUFFER_SIZE as u64)
        .ws_max_message_size(MAX_HTTP_BUFFER_SIZE)
        .ws_max_frame_size(MAX_HTTP_BUFFER_SIZE)
        .build_layer();

    // El namespace raíz no se usa
    io.ns("/", (|| ready(())).with(|| ready(Err::<(), _>(NO_AUTORIZADO))));

    // Un namespace por cada servicio permitido; cualquier otro es inválido
    for service in &config.allowed_services {
        let on_connect = {
            let registry = registry.clone();
            move |socket: SocketRef| -> Ready<()> {
                on_connection(socket, &registry);
                ready(())
            }
        };
        let authenticate = {
            let config = config.clone();
            move |socket: SocketRef, TryData(auth): TryData<Value>| -> Ready<Result<(), &'static str>> {
                let service = service_name(socket.ns());
                let token = auth
                    .as_ref()
                    .ok()
                    .and_then(|auth| auth.get("token"))
                    .and_then(Value::as_str);
                let valid =
                    token.is_some_and(|token| is_valid_socket_token(&config.secret, service, token, now_seconds()));
                if !valid {
                    return ready(Err(NO_AUTORIZADO));
                }
                if over_connection_limit(&socket, config.max_connections_per_ip) {
                    return ready(Err(DEMASIADAS_CONEXIONES));
                }
                ready(Ok(()))
            }
        };
        io.ns(format!("/{service}"), on_connect.with(authenticate));
    }

    (layer, io)
}

// IP (agrupada) del cliente según la petición del handshake de este socket
fn socket_ip(socket: &SocketRef) -> Option<IpAddr> {
    let parts = socket.req_parts();
    client_ip(&parts.headers, peer_ip(&parts.extensions)).map(limit_key)
}

// ¿Esta IP ya tiene el máximo de conexiones abiertas en el namespace? Se cuentan en
// vivo los sockets conectados: el que entra todavía no está en la lista. Sin IP
// conocida (o con tope 0) no se limita.
fn over_connection_limit(socket: &SocketRef, max: u32) -> bool {
    let Some(ip) = socket_ip(socket).filter(|_| max > 0) else {
        return false;
    };
    let open = socket
        .broadcast()
        .sockets()
        .iter()
        .filter(|other| socket_ip(other) == Some(ip))
        .count();
    open >= max as usize
}

// Remover el '/' inicial del namespace
fn service_name(namespace: &str) -> &str {
    namespace.strip_prefix('/').unwrap_or(namespace)
}

fn on_connection(socket: SocketRef, registry: &Arc<Registry>) {
    let service: Arc<str> = Arc::from(service_name(socket.ns()));

    // Inicializar el servicio si no existe
    registry.ensure_service(&service);

    // Cada socket entra a una sala con su propio id: así /emit codifica el paquete una
    // sola vez y lo envía a la lista de sockets que coinciden
    socket.join(socket.id);

    // Evento para registrar filtros adicionales del cliente
    socket.on("register_filters", {
        let (registry, service) = (registry.clone(), service.clone());
        move |socket: SocketRef, TryData(filters): TryData<Value>| -> Ready<()> {
            match filters.ok().filter(is_js_object) {
                Some(filters) => {
                    // Los filtros incluyen automáticamente el servicio
                    let complete = complete_filters(&service, &filters);
                    registry.register(&service, socket.id, complete.clone());
                    send(
                        &socket,
                        "filters_registered",
                        json!({
                            "message": "Filtros registrados correctamente",
                            "service": &*service,
                            "filters": complete,
                        }),
                    );
                }
                None => send_error(&socket, FILTROS_INVALIDOS),
            }
            ready(())
        }
    });

    // Evento para actualizar filtros: si el cliente no se registró no pasa nada
    socket.on("update_filters", {
        let (registry, service) = (registry.clone(), service.clone());
        move |socket: SocketRef, TryData(filters): TryData<Value>| -> Ready<()> {
            match filters.ok().filter(is_js_object) {
                Some(filters) => {
                    let complete = complete_filters(&service, &filters);
                    if registry.update(&service, socket.id, complete.clone()) {
                        send(
                            &socket,
                            "filters_updated",
                            json!({
                                "message": "Filtros actualizados correctamente",
                                "service": &*service,
                                "filters": complete,
                            }),
                        );
                    }
                }
                None => send_error(&socket, FILTROS_INVALIDOS),
            }
            ready(())
        }
    });

    // Evento para obtener filtros actuales
    socket.on("get_filters", {
        let (registry, service) = (registry.clone(), service.clone());
        move |socket: SocketRef| -> Ready<()> {
            match registry.filters_of(&service, socket.id) {
                Some(filters) => send(
                    &socket,
                    "current_filters",
                    json!({ "service": &*service, "filters": filters }),
                ),
                None => send_error(&socket, SIN_FILTROS),
            }
            ready(())
        }
    });

    // Manejar desconexión
    socket.on_disconnect({
        let (registry, service) = (registry.clone(), service.clone());
        move |socket: SocketRef| -> Ready<()> {
            registry.remove(&service, socket.id);
            ready(())
        }
    });
}

// Si el socket ya se cerró o su buffer está lleno, el mensaje se descarta
fn send(socket: &SocketRef, event: &str, payload: Value) {
    let _ = socket.emit(event, &payload);
}

fn send_error(socket: &SocketRef, message: &str) {
    send(socket, "error", json!({ "message": message }));
}

// Envía el evento a los sockets indicados con un solo paquete codificado.
// Un socket que falla no afecta a los demás.
pub async fn emit_to(io: &SocketIo, service: &str, sids: &[Sid], event: &str, payload: &Value) {
    let Some(namespace) = io.of(format!("/{service}")) else {
        return;
    };
    let rooms: Vec<String> = sids.iter().map(Sid::to_string).collect();
    let _ = namespace.to(rooms).emit(event, payload).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nombre_del_servicio() {
        assert_eq!(service_name("/devex"), "devex");
        assert_eq!(service_name("/"), "");
    }

    #[test]
    fn eventos_reservados() {
        assert!(is_reserved_event("connect"));
        assert!(is_reserved_event("removeListener"));
        assert!(!is_reserved_event("error"));
        assert!(!is_reserved_event("activities_changed"));
    }
}
