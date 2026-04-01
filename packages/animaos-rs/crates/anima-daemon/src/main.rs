use anima_daemon::Daemon;

fn main() -> std::io::Result<()> {
    let host = std::env::var("ANIMAOS_RS_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = std::env::var("ANIMAOS_RS_PORT").unwrap_or_else(|_| "8080".to_string());
    let bind_addr = format!("{host}:{port}");

    let daemon = Daemon::bind(bind_addr.as_str())?;
    let local_addr = daemon.local_addr()?;
    println!("anima-daemon listening on http://{local_addr}");

    daemon.serve()
}
