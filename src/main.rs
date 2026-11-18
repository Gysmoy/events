use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use socketioxide::SocketIo;
use tokio::net::TcpListener;
use tokio::sync::Notify;

use websocket_filter_service::app;
use websocket_filter_service::config::Config;

// Tiempo máximo para cerrar después de SIGTERM/SIGINT (docker espera 10 s antes de matar)
const SHUTDOWN_GRACE: Duration = Duration::from_millis(1000);

fn main() -> ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };

    // Un solo hilo: decenas de paneles conectados y unos pocos eventos por minuto no
    // necesitan más, y así no hay pila ni arena del allocator por cada worker
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("No se pudo iniciar el runtime: {error}");
            return ExitCode::FAILURE;
        }
    };

    let result = runtime.block_on(run(config));
    // Lo que quede pendiente (sockets cerrándose) se descarta sin esperar
    runtime.shutdown_timeout(Duration::from_millis(100));

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error del servidor: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(config: Config) -> std::io::Result<()> {
    let port = config.port;
    let services = config.allowed_services.join(", ");
    let origins = config.allowed_origins.join(", ");
    let (router, state) = app(config);

    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    println!("Servidor WebSocket ejecutándose en puerto {port}");
    println!("Servicios permitidos: {services}");
    println!("Orígenes permitidos: {origins}");

    let stopping = Arc::new(Notify::new());
    let server = axum::serve(listener, router).with_graceful_shutdown(shutdown(state.io, stopping.clone()));

    // Cierre ordenado, pero con tope: si algo queda colgado se sale igual
    tokio::select! {
        result = server => result,
        () = async {
            stopping.notified().await;
            tokio::time::sleep(SHUTDOWN_GRACE).await;
        } => Ok(()),
    }
}

// Espera SIGTERM o SIGINT y cierra todos los sockets
async fn shutdown(io: SocketIo, stopping: Arc<Notify>) {
    wait_for_signal().await;
    println!("Cerrando servidor");
    stopping.notify_one();
    io.close().await;
}

#[cfg(unix)]
async fn wait_for_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    match (signal(SignalKind::terminate()), signal(SignalKind::interrupt())) {
        (Ok(mut term), Ok(mut int)) => {
            tokio::select! {
                _ = term.recv() => {}
                _ = int.recv() => {}
            }
        }
        _ => {
            let _ = tokio::signal::ctrl_c().await;
        }
    }
}

#[cfg(not(unix))]
async fn wait_for_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
