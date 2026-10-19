// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use tiny_http::{Header, Method, Response, Server, StatusCode};

use crate::invalid;

fn resolve(root: &Path, url: &str) -> io::Result<PathBuf> {
    let path = url.split('?').next().unwrap_or("");
    if !path.starts_with('/') || path.contains('%') || path.contains('\\') {
        return Err(invalid("invalid asset path"));
    }
    let path = if path == "/" { "vnc.html" } else { &path[1..] };
    let full = root.join(path).canonicalize()?;
    if !full.starts_with(root) || !full.is_file() {
        return Err(invalid("asset outside web directory"));
    }
    Ok(full)
}

pub fn serve(address: SocketAddr, root: PathBuf) -> io::Result<()> {
    if !address.ip().is_loopback() {
        return Err(invalid("asset server must bind to loopback"));
    }
    let root = root.canonicalize()?;
    if !root.join("vnc.html").is_file() {
        return Err(invalid("asset directory must contain vnc.html"));
    }
    let server = Server::http(address).map_err(io::Error::other)?;
    eprintln!("noVNC assets on http://{address}");
    for request in server.incoming_requests() {
        let response = (|| {
            if request.method() != &Method::Get && request.method() != &Method::Head {
                return Err(invalid("method not supported"));
            }
            let path = resolve(&root, request.url())?;
            let file = std::fs::File::open(&path)?;
            if file.metadata()?.len() > 8 * 1024 * 1024 {
                return Err(invalid("asset too large"));
            }
            let content_type = match path.extension().and_then(|value| value.to_str()) {
                Some("html") => "text/html; charset=utf-8",
                Some("js") => "text/javascript; charset=utf-8",
                Some("css") => "text/css; charset=utf-8",
                Some("svg") => "image/svg+xml",
                Some("png") => "image/png",
                Some("ico") => "image/x-icon",
                Some("woff") => "font/woff",
                Some("woff2") => "font/woff2",
                Some("json") => "application/json",
                _ => "application/octet-stream",
            };
            Ok(Response::from_file(file)
                .with_header(Header::from_bytes("Content-Type", content_type).unwrap())
                .with_header(Header::from_bytes("X-Content-Type-Options", "nosniff").unwrap())
                .with_header(Header::from_bytes("Referrer-Policy", "no-referrer").unwrap())
                .with_header(Header::from_bytes("Cache-Control", "no-store").unwrap()))
        })();
        match response {
            Ok(response) => {
                let _ = request.respond(response);
            }
            Err(_) => {
                let _ = request.respond(Response::empty(StatusCode(404)));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn asset_paths_cannot_escape_root() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tools/windows/console/web")
            .canonicalize()
            .unwrap();
        assert_eq!(resolve(&root, "/").unwrap(), root.join("vnc.html"));
        resolve(&root, "/../Cargo.toml").unwrap_err();
        resolve(&root, "/%2e%2e/Cargo.toml").unwrap_err();
        resolve(&root, "//etc/passwd").unwrap_err();
    }
}
