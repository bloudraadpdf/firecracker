// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Runs an echo endpoint behind the Tokoloshe socket-grant proxy for guest tests.
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener},
    path::PathBuf,
    time::{Duration, Instant},
};
use tokoloshe_egress::{EgressPosture, EgressProxy, EgressSocketGrant};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let [run, sidecar, socket, endpoint] = args.as_slice() else {
        return Err("usage: windows_probe RUN SIDECAR SOCKET IPV4_ENDPOINT".into());
    };
    let run = PathBuf::from(run);
    std::fs::create_dir_all(run.join("jails"))?;
    let upstream = TcpListener::bind(endpoint)?;
    let SocketAddr::V4(endpoint) = upstream.local_addr()? else {
        return Err("IPv4 required".into());
    };
    std::thread::spawn(move || {
        for connection in upstream.incoming() {
            let Ok(mut stream) = connection else { break };
            std::thread::spawn(move || {
                if stream
                    .set_read_timeout(Some(Duration::from_secs(60)))
                    .is_err()
                    || stream
                        .set_write_timeout(Some(Duration::from_secs(60)))
                        .is_err()
                {
                    return;
                }
                let mut buf = [0; 8192];
                while let Ok(n) = stream.read(&mut buf) {
                    if n == 0 || stream.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
            });
        }
    });
    let posture = EgressPosture::pinned(
        "127.0.0.1:0".parse()?,
        Duration::from_secs(10),
        Duration::from_secs(60),
        Duration::from_secs(5),
    );
    let proxy = EgressProxy::launch(
        &posture,
        std::path::Path::new(sidecar),
        &run,
        &run.join("jails"),
    )?;
    let grant = EgressSocketGrant::new(
        uuid::Uuid::from_u128(1),
        rustix::process::geteuid().as_raw(),
        rustix::process::getegid().as_raw(),
        vec![endpoint],
    )?;
    let generation = proxy.open_socket(&grant, Instant::now() + Duration::from_secs(5))?;
    std::os::unix::fs::symlink(generation.socket(), socket)?;
    println!(
        "READY endpoint={endpoint} socket={}",
        generation.socket().display()
    );
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    println!(
        "ACCOUNTING {:?}",
        generation
            .seal()?
            .into_entries()
            .ok_or("expected entry accounting")?
    );
    std::fs::remove_file(socket)?;
    proxy.shutdown()?;
    Ok(())
}
