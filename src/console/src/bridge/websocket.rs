// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::io::{self, Cursor, Read, Write};

use tungstenite::handshake::server::{Request, Response};
use tungstenite::{Message, WebSocket};

use crate::invalid;

#[derive(Clone)]
pub struct Auth {
    token: String,
    origin: String,
}
impl Auth {
    pub fn new(token: String, origin: String) -> io::Result<Self> {
        if token.len() < 32
            || token.len() > 128
            || !token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return Err(invalid(
                "token must contain 32..128 random ASCII letters, digits, '-' or '_'",
            ));
        }
        if !(origin.starts_with("http://") || origin.starts_with("https://"))
            || origin.ends_with('/')
        {
            return Err(invalid(
                "origin must be the exact http(s) browser origin without trailing slash",
            ));
        }
        Ok(Self { token, origin })
    }

    fn allowed(&self, request: &Request) -> bool {
        let expected = format!("/console?token={}", self.token);
        let path = request
            .uri()
            .path_and_query()
            .map_or("", |value| value.as_str());
        let token_matches = path.len() == expected.len()
            && path
                .bytes()
                .zip(expected.bytes())
                .fold(0, |diff, (left, right)| diff | left ^ right)
                == 0;
        token_matches
            && request
                .headers()
                .get("origin")
                .and_then(|value| value.to_str().ok())
                == Some(self.origin.as_str())
    }
}

impl tungstenite::handshake::server::Callback for Auth {
    fn on_request(
        self,
        request: &Request,
        mut response: Response,
    ) -> Result<Response, tungstenite::handshake::server::ErrorResponse> {
        if !self.allowed(request) {
            return Err(Response::builder()
                .status(403)
                .body(Some("Forbidden".to_owned()))
                .unwrap());
        }
        if request
            .headers()
            .get("sec-websocket-protocol")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.split(',').any(|part| part.trim() == "binary"))
        {
            response
                .headers_mut()
                .insert("sec-websocket-protocol", "binary".parse().unwrap());
        }
        Ok(response)
    }
}

pub struct Stream {
    socket: WebSocket<super::transport::Stream>,
    pending: Cursor<Vec<u8>>,
}

pub fn accept(stream: super::transport::Stream, auth: &Auth) -> io::Result<Stream> {
    let config = tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(1024 * 1024))
        .max_frame_size(Some(1024 * 1024));
    let socket = tungstenite::accept_hdr_with_config(stream, auth.clone(), Some(config))
        .map_err(io::Error::other)?;
    Ok(Stream {
        socket,
        pending: Cursor::new(Vec::new()),
    })
}

impl Read for Stream {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        loop {
            let count = self.pending.read(output)?;
            if count != 0 {
                return Ok(count);
            }
            match self.socket.read().map_err(io::Error::other)? {
                Message::Binary(bytes) => self.pending = Cursor::new(bytes.to_vec()),
                Message::Close(_) => return Ok(0),
                Message::Ping(_) | Message::Pong(_) => {
                    self.socket.flush().map_err(io::Error::other)?
                }
                _ => return Err(invalid("RFB requires binary WebSocket messages")),
            }
        }
    }
}

impl Write for Stream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.socket
            .send(Message::Binary(bytes.to_vec().into()))
            .map_err(io::Error::other)?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.socket.flush().map_err(io::Error::other)
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpStream;

    use super::*;
    #[test]
    fn origin_and_token_are_both_required() {
        let token = "a".repeat(32);
        let auth = Auth::new(token.clone(), "https://console.example".to_owned()).unwrap();
        let request = |origin: &str, suffix: &str| {
            Request::builder()
                .uri(format!("/console?token={suffix}"))
                .header("Origin", origin)
                .body(())
                .unwrap()
        };
        assert!(auth.allowed(&request("https://console.example", &token)));
        assert!(!auth.allowed(&request("https://attacker.example", &token)));
        assert!(!auth.allowed(&request("https://console.example", "wrong")));
        assert!(
            !auth.allowed(
                &Request::builder()
                    .uri(format!("/console?token={token}"))
                    .body(())
                    .unwrap()
            )
        );
    }
    #[test]
    fn authenticated_websocket_carries_binary_rfb_stream() {
        use tungstenite::client::IntoClientRequest;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let token = "b".repeat(32);
        let auth = Auth::new(token.clone(), "https://console.example".to_owned()).unwrap();
        let server = std::thread::spawn(move || {
            let (tcp, _) = listener.accept().unwrap();
            crate::bridge::configure(&tcp).unwrap();
            let mut stream = accept(crate::bridge::transport::Stream::Tcp(tcp), &auth).unwrap();
            stream.write_all(b"RFB 003.008\n").unwrap();
            let mut reply = [0; 12];
            stream.read_exact(&mut reply).unwrap();
            assert_eq!(&reply, b"RFB 003.008\n");
        });
        let mut request = format!("ws://{address}/console?token={token}")
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("origin", "https://console.example".parse().unwrap());
        let tcp = TcpStream::connect(address).unwrap();
        crate::bridge::configure(&tcp).unwrap();
        let (mut client, _) = tungstenite::client(request, tcp).unwrap();
        assert_eq!(
            client.read().unwrap().into_data().as_ref(),
            b"RFB 003.008\n"
        );
        client
            .send(Message::Binary(b"RFB ".to_vec().into()))
            .unwrap();
        client
            .send(Message::Binary(b"003.008\n".to_vec().into()))
            .unwrap();
        server.join().unwrap();
    }
}
