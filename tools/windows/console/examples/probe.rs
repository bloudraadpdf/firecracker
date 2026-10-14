// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Live authenticated WebSocket/RFB probe. Sends and releases the W key.

use std::collections::VecDeque;
use std::io;
use std::net::TcpStream;
use std::time::Duration;

use tungstenite::client::IntoClientRequest;
use tungstenite::{Message, WebSocket};

struct Client {
    socket: WebSocket<TcpStream>,
    pending: VecDeque<u8>,
}

impl Client {
    fn receive(&mut self, count: usize) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        while self.pending.len() < count {
            match self.socket.read()? {
                Message::Binary(data) => self.pending.extend(data),
                Message::Ping(_) | Message::Pong(_) => self.socket.flush()?,
                message => {
                    return Err(io::Error::other(format!("unexpected message {message:?}")).into());
                }
            }
        }
        Ok(self.pending.drain(..count).collect())
    }
    fn send(&mut self, bytes: &[u8]) -> Result<(), tungstenite::Error> {
        self.socket.send(Message::Binary(bytes.to_vec().into()))
    }
}

fn connect(address: &str, token: &str, origin: &str) -> Result<Client, Box<dyn std::error::Error>> {
    let mut request = format!("ws://{address}/console?token={token}").into_client_request()?;
    request.headers_mut().insert("origin", origin.parse()?);
    let stream = TcpStream::connect(address)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    let (socket, _) = tungstenite::client(request, stream)?;
    Ok(Client {
        socket,
        pending: VecDeque::new(),
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        return Err(io::Error::other("usage: probe ADDRESS TOKEN_FILE ORIGIN").into());
    }
    let token = std::fs::read_to_string(&args[1])?;
    let mut client = connect(&args[0], token.trim(), &args[2])?;
    assert_eq!(client.receive(12)?, b"RFB 003.008\n");
    client.send(b"RFB 003.008\n")?;
    assert_eq!(client.receive(2)?, [1, 1]);
    client.send(&[1])?;
    assert_eq!(client.receive(4)?, [0; 4]);
    client.send(&[1])?;
    let init = client.receive(24)?;
    let width = u16::from_be_bytes(init[..2].try_into()?);
    let height = u16::from_be_bytes(init[2..4].try_into()?);
    let name_length = u32::from_be_bytes(init[20..24].try_into()?);
    if name_length > 1024 {
        return Err(io::Error::other("invalid desktop name length").into());
    }
    let name = client.receive(name_length as usize)?;
    let mut update = vec![3, 0, 0, 0, 0, 0];
    update.extend(width.to_be_bytes());
    update.extend(height.to_be_bytes());
    client.send(&update)?;
    assert_eq!(client.receive(4)?, [0, 0, 0, 1]);
    let rectangle = client.receive(12)?;
    assert_eq!(&rectangle[..4], &[0; 4]);
    assert_eq!(&rectangle[8..], &[0; 4]);
    let rectangle_width = u16::from_be_bytes(rectangle[4..6].try_into()?);
    let rectangle_height = u16::from_be_bytes(rectangle[6..8].try_into()?);
    assert_eq!((rectangle_width, rectangle_height), (width, height));
    let length = usize::from(width) * usize::from(height) * 4;
    if length > 32 * 1024 * 1024 {
        return Err(io::Error::other("frame exceeds limit").into());
    }
    let pixels = client.receive(length)?;
    let nonzero = pixels.iter().filter(|&&value| value != 0).count();
    client.send(&[4, 1, 0, 0, 0, 0, 0, 0x77])?;
    client.send(&[4, 0, 0, 0, 0, 0, 0, 0x77])?;
    // A subsequent update confirms both preceding key messages were consumed.
    client.send(&update)?;
    assert_eq!(client.receive(4)?, [0, 0, 0, 1]);
    client.receive(12)?;
    client.receive(length)?;
    client.socket.close(None)?;
    println!(
        "PASS: authenticated RFB {} {}x{}, {} pixel bytes ({} nonzero), W press/release",
        String::from_utf8(name)?,
        width,
        height,
        length,
        nonzero
    );
    Ok(())
}
