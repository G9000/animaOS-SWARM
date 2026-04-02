use std::io;
use std::time::Duration;

use anima_daemon::{serve, DaemonConfig};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> io::Result<()> {
    let host = std::env::var("ANIMAOS_RS_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = std::env::var("ANIMAOS_RS_PORT").unwrap_or_else(|_| "8080".to_string());
    let bind_addr = format!("{host}:{port}");
    let default_config = DaemonConfig::default();
    let config = DaemonConfig {
        max_request_bytes: parse_env_usize(
            "ANIMAOS_RS_MAX_REQUEST_BYTES",
            default_config.max_request_bytes,
        )?,
        request_read_timeout: Duration::from_millis(parse_env_u64(
            "ANIMAOS_RS_REQUEST_READ_TIMEOUT_MS",
            default_config.request_read_timeout.as_millis() as u64,
        )?),
    };

    let listener = TcpListener::bind(bind_addr.as_str()).await?;
    let local_addr = listener.local_addr()?;
    println!("anima-daemon listening on http://{local_addr}");

    serve(listener, config).await
}

fn parse_env_usize(name: &str, default: usize) -> io::Result<usize> {
    match std::env::var(name) {
        Ok(value) => value.parse::<usize>().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{name} must be a positive integer"),
            )
        }),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("failed to read {name}: {error}"),
        )),
    }
}

fn parse_env_u64(name: &str, default: u64) -> io::Result<u64> {
    match std::env::var(name) {
        Ok(value) => value.parse::<u64>().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{name} must be a positive integer"),
            )
        }),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("failed to read {name}: {error}"),
        )),
    }
}
