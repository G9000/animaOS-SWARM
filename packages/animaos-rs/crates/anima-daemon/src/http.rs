use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::str;

use crate::json::escape_json;
use crate::DaemonConfig;

pub(crate) struct Request {
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) query: HashMap<String, String>,
    pub(crate) body: Vec<u8>,
}

pub(crate) struct Response {
    pub(crate) status_line: &'static str,
    pub(crate) body: String,
}

impl Response {
    pub(crate) fn json(status_line: &'static str, body: String) -> Self {
        Self { status_line, body }
    }

    pub(crate) fn error(status_line: &'static str, message: &'static str) -> Self {
        Self::json(
            status_line,
            format!("{{\"error\":\"{}\"}}", escape_json(message)),
        )
    }
}

pub(crate) fn prepare_stream(stream: &TcpStream, config: DaemonConfig) -> io::Result<()> {
    stream.set_read_timeout(Some(config.request_read_timeout))
}

pub(crate) fn write_http_response(stream: &mut TcpStream, response: Response) -> io::Result<()> {
    let http = format!(
        "{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        response.status_line,
        response.body.len(),
        response.body
    );
    stream.write_all(http.as_bytes())?;
    stream.flush()
}

pub(crate) fn read_http_request(
    stream: &mut TcpStream,
    config: DaemonConfig,
) -> io::Result<Vec<u8>> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 1024];
    let mut header_end = None;
    let mut content_length = 0;

    loop {
        let bytes_read = stream.read(&mut chunk)?;
        if bytes_read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..bytes_read]);
        if buffer.len() > config.max_request_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request exceeds maximum size",
            ));
        }

        if header_end.is_none() {
            if let Some(end) = find_sequence(&buffer, b"\r\n\r\n") {
                header_end = Some(end + 4);
                content_length = parse_content_length(&buffer[..end + 4])?;
                if end + 4 + content_length > config.max_request_bytes {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "request exceeds maximum size",
                    ));
                }
            }
        }

        if let Some(end) = header_end {
            if buffer.len() >= end + content_length {
                break;
            }
        }
    }

    if buffer.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "empty request",
        ));
    }
    if let Some(end) = header_end {
        if buffer.len() < end + content_length {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "request body incomplete",
            ));
        }
    }

    Ok(buffer)
}

pub(crate) fn parse_request(buffer: &[u8]) -> io::Result<Request> {
    let header_end = find_sequence(buffer, b"\r\n\r\n")
        .map(|index| index + 4)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "request missing header terminator",
            )
        })?;
    let header = str::from_utf8(&buffer[..header_end])
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "request header is not utf-8"))?;

    let mut lines = header.lines();
    let request_line = lines
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "request line missing"))?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "request method missing"))?
        .to_string();
    let target = request_parts
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "request target missing"))?;
    let (path, query) = match target.split_once('?') {
        Some((path, query)) => (path.to_string(), parse_query_string(query)?),
        None => (target.to_string(), HashMap::new()),
    };

    Ok(Request {
        method,
        path,
        query,
        body: buffer[header_end..].to_vec(),
    })
}

fn parse_content_length(header: &[u8]) -> io::Result<usize> {
    let header = str::from_utf8(header)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "header is not utf-8"))?;
    for line in header.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("content-length") {
            return value
                .trim()
                .parse::<usize>()
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid content length"));
        }
    }
    Ok(0)
}

fn find_sequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn parse_query_string(query: &str) -> io::Result<HashMap<String, String>> {
    let mut params = HashMap::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        params.insert(percent_decode(key)?, percent_decode(value)?);
    }
    Ok(params)
}

fn percent_decode(value: &str) -> io::Result<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' => {
                if index + 2 >= bytes.len() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid percent-encoding",
                    ));
                }
                let value = decode_hex_byte(bytes[index + 1], bytes[index + 2])?;
                decoded.push(value);
                index += 3;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }

    String::from_utf8(decoded)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "query parameter is not utf-8"))
}

fn decode_hex_byte(left: u8, right: u8) -> io::Result<u8> {
    Ok((hex_value(left)? << 4) | hex_value(right)?)
}

fn hex_value(byte: u8) -> io::Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid percent-encoding",
        )),
    }
}
