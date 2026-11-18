//! Servicio WebSocket con sistema de filtros para notificaciones dirigidas.
//! Los backends emiten por HTTP y el evento solo llega a los clientes conectados
//! cuyos filtros coinciden.

pub mod auth;
pub mod body;
pub mod config;
pub mod filters;
pub mod http;
pub mod limits;
pub mod sockets;
pub mod state;

use std::sync::Arc;

use axum::Router;
use socketioxide::SocketIo;

use crate::config::Config;
use crate::limits::HandshakeLimiter;
use crate::state::Registry;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub registry: Arc<Registry>,
    pub io: SocketIo,
    pub handshakes: Arc<HandshakeLimiter>,
}

// Arma el router completo (REST + Socket.IO) y devuelve el estado compartido
pub fn app(config: Config) -> (Router, AppState) {
    let config = Arc::new(config);
    let registry = Arc::new(Registry::default());
    let handshakes = Arc::new(HandshakeLimiter::new(config.handshakes_per_minute_per_ip));
    let (socket_layer, io) = sockets::layer(&config, &registry);
    let state = AppState {
        config,
        registry,
        io,
        handshakes,
    };
    let router = http::router(state.clone(), socket_layer);
    (router, state)
}
