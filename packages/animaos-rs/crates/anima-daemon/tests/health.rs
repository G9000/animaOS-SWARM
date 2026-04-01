use std::io::{Read, Write};
use std::net::TcpStream;
use std::thread;
use std::time::Duration;

use anima_daemon::Daemon;
use futures::executor::block_on;

#[test]
fn health_endpoint_returns_ok_json() {
    let daemon = Daemon::bind("127.0.0.1:0").expect("daemon binds");
    let addr = daemon.local_addr().expect("daemon reports local addr");

    let server = thread::spawn(move || {
        block_on(daemon.serve_one()).expect("daemon serves one request");
    });

    thread::sleep(Duration::from_millis(25));

    let mut stream = TcpStream::connect(addr).expect("client connects");
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .expect("request written");

    let mut response = String::new();
    stream.read_to_string(&mut response).expect("response read");

    server.join().expect("server thread joins");

    assert!(
        response.starts_with("HTTP/1.1 200 OK"),
        "unexpected response: {response}"
    );
    assert!(
        response.contains("{\"status\":\"ok\"}"),
        "health body missing ok status: {response}"
    );
}
