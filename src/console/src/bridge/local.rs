// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::net::{Shutdown, TcpListener};
use std::sync::Mutex;
use std::thread::JoinHandle;

use super::*;

/// An in-process, loopback-only RFB bridge owned by a local GUI launch.
#[derive(Debug)]
pub struct LocalBridge {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    peer: Arc<Mutex<Option<TcpStream>>>,
    thread: Option<JoinHandle<()>>,
}

impl LocalBridge {
    /// Start a bridge for an existing private Firecracker console socket.
    pub fn start(socket: PathBuf) -> io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let peer = Arc::new(Mutex::new(None));
        let cancelled = stop.clone();
        let active = peer.clone();
        let thread = std::thread::Builder::new()
            .name("console-bridge".into())
            .spawn(move || {
                let busy = Arc::new(AtomicBool::new(true));
                while !cancelled.load(Ordering::Acquire) {
                    let stream = match listener.accept() {
                        Ok((stream, _)) => stream,
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(10));
                            continue;
                        }
                        Err(error) => {
                            eprintln!("Console accept failed: {error}");
                            break;
                        }
                    };
                    let result = (|| {
                        let mut peer = active.lock().unwrap();
                        if cancelled.load(Ordering::Acquire) {
                            return Ok(());
                        }
                        *peer = Some(stream.try_clone()?);
                        drop(peer);
                        let mut guard = SessionGuard {
                            access: crate::bridge::rfb::Access::Control,
                            input_active: false,
                            backend: Backend(socket.clone()),
                            busy: busy.clone(),
                        };
                        serve_connection(
                            transport::Stream::Tcp(stream),
                            &mut guard,
                            None,
                            None,
                            rfb::KeyboardMapping::Standard,
                        )
                    })();
                    active.lock().unwrap().take();
                    if let Err(error) = result
                        && !cancelled.load(Ordering::Acquire)
                    {
                        eprintln!("Console session ended: {error}");
                    }
                }
            })?;
        Ok(Self {
            address,
            stop,
            peer,
            thread: Some(thread),
        })
    }

    /// The allocated loopback address, for this launch only.
    pub fn address(&self) -> SocketAddr {
        self.address
    }
}

impl Drop for LocalBridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(peer) = self.peer.lock().unwrap().as_ref() {
            let _ = peer.shutdown(Shutdown::Both);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;

    use super::*;

    #[test]
    fn native_client_drives_bridge_and_disconnect_releases_guest_input() {
        let path = PathBuf::from(format!("gui-client-{}.sock", std::process::id()));
        let listener = UnixListener::bind(&path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let finished = stop.clone();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let backend = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while !finished.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
                let (mut stream, _) = match listener.accept() {
                    Ok(peer) => peer,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(1));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = [0; 12];
                stream.read_exact(&mut request).unwrap();
                let response = if request[4] == 0 {
                    let mut response = b"FCF1".to_vec();
                    for n in [2u32, 1, 8] {
                        response.extend(n.to_le_bytes());
                    }
                    response.extend([3, 2, 1, 0, 6, 5, 4, 0]);
                    response
                } else {
                    let mut response = b"FCA1".to_vec();
                    response.extend([0; 12]);
                    if request[4] == 5 {
                        response[8] = 1;
                    }
                    response
                };
                stream.write_all(&response).unwrap();
                recorded.lock().unwrap().push(request);
            }
        });
        let bridge = LocalBridge::start(path.clone()).unwrap();
        let mut peer = TcpStream::connect(bridge.address()).unwrap();
        configure(&peer).unwrap();
        let mut frame = crate::client::handshake(&mut peer, None).unwrap();
        crate::client::request(&mut peer, (frame.width, frame.height)).unwrap();
        let frame = crate::client::frame(&mut peer, &mut frame).unwrap();
        assert_eq!(frame.pixels, [3, 2, 1, 0, 6, 5, 4, 0]);
        crate::client::key(&mut peer, 0xffeb, true).unwrap();
        crate::client::pointer(&mut peer, 1, 1, 0).unwrap();
        drop(peer);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r[4] == 3)
            .count()
            < 2
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(1));
        }
        drop(bridge);
        stop.store(true, Ordering::Release);
        backend.join().unwrap();
        std::fs::remove_file(path).unwrap();
        let requests = requests.lock().unwrap();
        assert!(requests.iter().any(|r| r[4..8] == [1, 0x1f, 1, 1]));
        assert!(
            requests
                .iter()
                .any(|r| r[4..11] == [6, 1, 255, 127, 0, 0, 0])
        );
        assert_eq!(requests.iter().filter(|r| r[4] == 3).count(), 2);
    }

    #[test]
    fn closing_bridge_cancels_idle_and_unauthenticated_sessions() {
        for connected in [false, true] {
            let bridge = LocalBridge::start(PathBuf::from("nonexistent-console.sock")).unwrap();
            let address = bridge.address();
            let peer = connected.then(|| TcpStream::connect(address).unwrap());
            if let Some(peer) = &peer {
                peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut greeting = [0; 12];
                (&*peer).read_exact(&mut greeting).unwrap();
                assert_eq!(&greeting, b"RFB 003.008\n");
            }
            let start = std::time::Instant::now();
            drop(bridge);
            assert!(start.elapsed() < Duration::from_secs(2));
            TcpStream::connect_timeout(&address, Duration::from_secs(1)).unwrap_err();
        }
    }
}
