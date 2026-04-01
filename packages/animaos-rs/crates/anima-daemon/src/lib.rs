use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};

use anima_core::HealthStatus;
use anima_memory::MemoryManager;
use anima_swarm::SwarmCoordinator;

const NOT_FOUND_JSON: &str = "{\"error\":\"not found\"}";

pub struct Daemon {
    listener: TcpListener,
    _memory: MemoryManager,
    _swarm: SwarmCoordinator,
}

impl Daemon {
    pub fn bind<A: ToSocketAddrs>(addr: A) -> std::io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        Ok(Self {
            listener,
            _memory: MemoryManager::new(),
            _swarm: SwarmCoordinator::new(),
        })
    }

    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    pub fn serve_one(self) -> std::io::Result<()> {
        let (stream, _) = self.listener.accept()?;
        handle_connection(stream)
    }

    pub fn serve(self) -> std::io::Result<()> {
        for stream in self.listener.incoming() {
            handle_connection(stream?)?;
        }
        Ok(())
    }
}

fn handle_connection(mut stream: TcpStream) -> std::io::Result<()> {
    let mut buffer = [0_u8; 1024];
    let bytes_read = stream.read(&mut buffer)?;
    let request = String::from_utf8_lossy(&buffer[..bytes_read]);

    let (status_line, body) = if is_health_request(&request) {
        ("HTTP/1.1 200 OK", HealthStatus::ok().as_json())
    } else {
        ("HTTP/1.1 404 Not Found", NOT_FOUND_JSON)
    };

    let response = format!(
        "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );

    stream.write_all(response.as_bytes())?;
    stream.flush()?;
    Ok(())
}

fn is_health_request(request: &str) -> bool {
    request
        .lines()
        .next()
        .is_some_and(|line| line.starts_with("GET /health "))
}
