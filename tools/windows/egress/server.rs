// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn received_at() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("host clock precedes Unix epoch")
        .as_millis()
}

fn reports(address: Ipv4Addr) -> std::io::Result<()> {
    for connection in TcpListener::bind((address, 40570))?.incoming() {
        let stream = connection?;
        stream.set_read_timeout(Some(Duration::from_secs(30)))?;
        let mut message = String::new();
        stream.take(1024).read_to_string(&mut message)?;
        println!("report: {} host_unix_ms={}", message.trim(), received_at());
    }
    Ok(())
}

fn main() -> std::io::Result<()> {
    let timeout = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "120".into())
        .parse::<u64>()
        .map(Duration::from_secs)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    let address = std::env::args()
        .nth(1)
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "expected listen IPv4 address",
            )
        })?
        .parse::<Ipv4Addr>()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    std::thread::spawn(move || {
        if let Err(error) = reports(address) {
            eprintln!("report listener failed: {error}");
            std::process::exit(1);
        }
    });
    for connection in TcpListener::bind((address, 40569))?.incoming() {
        let stream = connection?;
        let peer = stream.peer_addr()?;
        let total = echo(stream, timeout)?;
        println!(
            "received: echo peer={peer} host_unix_ms={}\necho: peer={peer} bytes={total}",
            received_at()
        );
    }
    Ok(())
}

fn echo(mut stream: std::net::TcpStream, timeout: Duration) -> std::io::Result<usize> {
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let mut total = 0;
    let mut buffer = [0; 8192];
    loop {
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            return Ok(total);
        }
        stream.write_all(&buffer[..count])?;
        total += count;
    }
}
