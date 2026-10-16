// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

fn exchange(stream: &mut UnixStream, opcode: u16, payload: &[u8]) -> io::Result<Vec<u8>> {
    let mut header = *b"FCUV\x01\0\0\0\x01\0\0\0\0\0\0\0";
    header[6..8].copy_from_slice(&opcode.to_le_bytes());
    header[12..].copy_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
    stream.write_all(&header)?;
    stream.write_all(payload)?;
    let mut response = [0; 16];
    stream.read_exact(&mut response)?;
    header[6..8].copy_from_slice(&(opcode | 0x8000).to_le_bytes());
    if response[..12] != header[..12] {
        return Err(io::Error::other("invalid response header"));
    }
    let size = u32::from_le_bytes(response[12..].try_into().unwrap());
    if !(4..=4096).contains(&size) {
        return Err(io::Error::other("invalid response size"));
    }
    let mut bytes = vec![0; usize::try_from(size).unwrap()];
    stream.read_exact(&mut bytes)?;
    if bytes[..4] != [0; 4] {
        return Err(io::Error::other("helper rejected request"));
    }
    Ok(bytes[4..].to_vec())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 {
        return Err("usage: ppi-submit SOCKET OPERATION".into());
    }
    let operation: u32 = args[2].parse()?;
    let mut stream = UnixStream::connect(&args[1])?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    exchange(&mut stream, 4, &[])?;
    let request: Vec<_> = [2_u32, 1, operation, 0]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    let response = exchange(&mut stream, 5, &request)?;
    if response != [0; 12] {
        return Err(format!("PPI submission failed: {response:?}").into());
    }
    println!("PPI operation {operation} submitted");
    Ok(())
}
