use std::io;
use std::time::Duration;

use anima_daemon::{serve, DaemonConfig, PersistenceMode};
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> io::Result<()> {
    init_tracing();

    let host = std::env::var("ANIMAOS_RS_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = std::env::var("ANIMAOS_RS_PORT").unwrap_or_else(|_| "8080".to_string());
    let bind_addr = format!("{host}:{port}");
    let default_config = DaemonConfig::default();
    let config = DaemonConfig {
        max_request_bytes: parse_env_usize(
            "ANIMAOS_RS_MAX_REQUEST_BYTES",
            default_config.max_request_bytes,
        )?,
        request_timeout: Duration::from_secs(parse_env_u64(
            "ANIMAOS_RS_REQUEST_TIMEOUT_SECS",
            default_config.request_timeout.as_secs(),
        )?),
        persistence_mode: parse_persistence_mode(default_config.persistence_mode)?,
        max_concurrent_runs: parse_env_usize(
            "ANIMAOS_RS_MAX_CONCURRENT_RUNS",
            default_config.max_concurrent_runs,
        )?,
        max_background_processes: parse_env_usize(
            "ANIMAOS_RS_MAX_BACKGROUND_PROCESSES",
            default_config.max_background_processes,
        )?,
    };

    let listener = TcpListener::bind(bind_addr.as_str()).await?;
    let local_addr = listener.local_addr()?;
    info!(
        address = %local_addr,
        timeout_secs = config.request_timeout.as_secs(),
        persistence_mode = config.persistence_mode.as_str(),
        max_concurrent_runs = config.max_concurrent_runs,
        max_background_processes = config.max_background_processes,
        runtime_memory_store = runtime_memory_store_label(),
        control_plane_durability = control_plane_store_label(),
        "anima-daemon listening"
    );

    serve(listener, config).await
}

fn control_plane_store_label() -> String {
    if let Ok(path) = std::env::var("ANIMAOS_RS_CONTROL_PLANE_FILE") {
        return format!("json:{path}");
    }
    "ephemeral".to_string()
}

fn runtime_memory_store_label() -> String {
    if let Ok(path) = std::env::var("ANIMAOS_RS_MEMORY_SQLITE_FILE") {
        return format!("sqlite:{path}");
    }
    if let Ok(path) = std::env::var("ANIMAOS_RS_MEMORY_FILE") {
        return format!("json:{path}");
    }
    "memory-only".to_string()
}

fn parse_env_usize(name: &str, default: usize) -> io::Result<usize> {
    match std::env::var(name) {
        Ok(value) => {
            let parsed = value.parse::<usize>().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{name} must be a positive integer"),
                )
            })?;
            if parsed == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{name} must be a positive integer"),
                ));
            }
            Ok(parsed)
        }
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("failed to read {name}: {error}"),
        )),
    }
}

fn parse_env_u64(name: &str, default: u64) -> io::Result<u64> {
    match std::env::var(name) {
        Ok(value) => {
            let parsed = value.parse::<u64>().map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{name} must be a positive integer"),
                )
            })?;
            if parsed == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{name} must be a positive integer"),
                ));
            }
            Ok(parsed)
        }
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("failed to read {name}: {error}"),
        )),
    }
}

fn parse_persistence_mode(default: PersistenceMode) -> io::Result<PersistenceMode> {
    match std::env::var("ANIMAOS_RS_PERSISTENCE_MODE") {
        Ok(value) => match value.to_ascii_lowercase().as_str() {
            "memory" => Ok(PersistenceMode::Memory),
            "postgres" => Ok(PersistenceMode::Postgres),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "ANIMAOS_RS_PERSISTENCE_MODE must be either 'memory' or 'postgres'",
            )),
        },
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("failed to read ANIMAOS_RS_PERSISTENCE_MODE: {error}"),
        )),
    }
}

fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("anima_daemon=info,tower_http=info"));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .compact()
        .try_init();
}
