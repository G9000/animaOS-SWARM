mod components;
mod http;
mod json;
mod model;
mod routes;
mod state;
mod tools;

use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::http::{
    parse_request, prepare_stream, read_http_request, write_http_response, Response,
};
use crate::routes::route_request;
use crate::state::DaemonState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DaemonConfig {
    pub max_request_bytes: usize,
    pub request_read_timeout: Duration,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            max_request_bytes: 64 * 1024,
            request_read_timeout: Duration::from_millis(200),
        }
    }
}

pub struct Daemon {
    listener: TcpListener,
    state: Arc<Mutex<DaemonState>>,
    config: DaemonConfig,
}

impl Daemon {
    pub fn bind<A: ToSocketAddrs>(addr: A) -> io::Result<Self> {
        Self::bind_with_config(addr, DaemonConfig::default())
    }

    pub fn bind_with_config<A: ToSocketAddrs>(addr: A, config: DaemonConfig) -> io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        Ok(Self {
            listener,
            state: Arc::new(Mutex::new(DaemonState::new())),
            config,
        })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    pub async fn serve_one(self) -> io::Result<()> {
        self.serve_n(1).await
    }

    pub async fn serve_n(self, limit: usize) -> io::Result<()> {
        for _ in 0..limit {
            let (stream, _) = self.listener.accept()?;
            handle_connection(stream, &self.state, self.config).await?;
        }
        Ok(())
    }

    pub async fn serve(self) -> io::Result<()> {
        for stream in self.listener.incoming() {
            handle_connection(stream?, &self.state, self.config).await?;
        }
        Ok(())
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    state: &Arc<Mutex<DaemonState>>,
    config: DaemonConfig,
) -> io::Result<()> {
    prepare_stream(&stream, config)?;
    let request_bytes = match read_http_request(&mut stream, config) {
        Ok(request_bytes) => request_bytes,
        Err(_) => {
            let _ = write_http_response(
                &mut stream,
                Response::error("HTTP/1.1 400 Bad Request", "malformed request"),
            );
            return Ok(());
        }
    };
    let request = match parse_request(&request_bytes) {
        Ok(request) => request,
        Err(_) => {
            let _ = write_http_response(
                &mut stream,
                Response::error("HTTP/1.1 400 Bad Request", "malformed request"),
            );
            return Ok(());
        }
    };
    let response = route_request(request, state).await;

    write_http_response(&mut stream, response)
}
