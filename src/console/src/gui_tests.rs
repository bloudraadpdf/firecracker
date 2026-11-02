// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::io::Write;
use std::net::TcpListener;
use std::time::Instant;

use super::*;

#[derive(Debug, PartialEq, Eq)]
enum Message {
    Refresh,
    Key(u32, bool),
    Pointer(u8, u16, u16),
}

struct Server {
    address: SocketAddr,
    peer: Arc<Mutex<Option<TcpStream>>>,
    messages: Arc<Mutex<Vec<Message>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let peer = Arc::new(Mutex::new(None));
        let messages = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let active = peer.clone();
        let recorded = messages.clone();
        let cancelled = stop.clone();
        let thread = std::thread::spawn(move || {
            while !cancelled.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(5)))
                            .unwrap();
                        stream
                            .set_write_timeout(Some(Duration::from_secs(5)))
                            .unwrap();
                        *active.lock().unwrap() = Some(stream.try_clone().unwrap());
                        let result = Self::serve(&mut stream, &recorded);
                        if let Err(error) = result {
                            assert!(
                                matches!(
                                    error.kind(),
                                    io::ErrorKind::UnexpectedEof
                                        | io::ErrorKind::BrokenPipe
                                        | io::ErrorKind::ConnectionReset
                                        | io::ErrorKind::ConnectionAborted
                                ),
                                "{error}"
                            );
                        }
                        return;
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("{error}"),
                }
            }
        });
        Self {
            address,
            peer,
            messages,
            stop,
            thread: Some(thread),
        }
    }

    fn options(&self) -> ViewerOptions {
        ViewerOptions {
            address: self.address,
            password_file: None,
        }
    }

    fn serve(stream: &mut TcpStream, messages: &Mutex<Vec<Message>>) -> io::Result<()> {
        stream.write_all(b"RFB 003.008\n")?;
        assert_eq!(crate::read::<12>(stream)?, *b"RFB 003.008\n");
        stream.write_all(&[1, 1])?;
        assert_eq!(crate::read::<1>(stream)?, [1]);
        stream.write_all(&[0; 4])?;
        assert_eq!(crate::read::<1>(stream)?, [1]);
        stream.write_all(&4u16.to_be_bytes())?;
        stream.write_all(&3u16.to_be_bytes())?;
        stream.write_all(&crate::RFB_FORMAT)?;
        stream.write_all(&0u32.to_be_bytes())?;
        loop {
            let message = match crate::read::<1>(stream)?[0] {
                0 => {
                    crate::read::<19>(stream)?;
                    continue;
                }
                2 => {
                    let header = crate::read::<3>(stream)?;
                    let count = u16::from_be_bytes([header[1], header[2]]);
                    for _ in 0..count {
                        crate::read::<4>(stream)?;
                    }
                    continue;
                }
                3 => {
                    crate::read::<9>(stream)?;
                    stream.write_all(&[0, 0, 0, 1])?;
                    stream.write_all(&[0, 0, 0, 0, 0, 4, 0, 3, 0, 0, 0, 0])?;
                    stream.write_all(&[0x7f; 48])?;
                    Message::Refresh
                }
                4 => {
                    let data = crate::read::<7>(stream)?;
                    Message::Key(
                        u32::from_be_bytes(data[3..].try_into().unwrap()),
                        data[0] != 0,
                    )
                }
                5 => {
                    let data = crate::read::<5>(stream)?;
                    Message::Pointer(
                        data[0],
                        u16::from_be_bytes([data[1], data[2]]),
                        u16::from_be_bytes([data[3], data[4]]),
                    )
                }
                other => panic!("interleaved or invalid RFB message: {other}"),
            };
            messages.lock().unwrap().push(message);
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(peer) = &*self.peer.lock().unwrap() {
            let _ = peer.shutdown(Shutdown::Both);
        }
        let result = self.thread.take().unwrap().join();
        if !std::thread::panicking() {
            result.unwrap();
        }
    }
}

#[test]
fn refresh_and_input_share_complete_wire_messages_and_cancel_on_drop() {
    let server = Server::new();
    let (events, received) = mpsc::channel();
    let (connection, _) = Connection::open(server.options(), move |event| {
        let _ = events.send(event);
    })
    .unwrap();
    for index in 0..200 {
        connection.commands.send(Command::Key(0x61, true)).unwrap();
        connection.commands.send(Command::Key(0x61, false)).unwrap();
        if index % 20 == 0 {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    connection
        .commands
        .send(Command::Pointer(1, (3, 2)))
        .unwrap();
    connection.commands.send(Command::Release).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let messages = server.messages.lock().unwrap();
        if messages.contains(&Message::Pointer(0, 3, 2)) {
            assert_eq!(
                messages
                    .iter()
                    .filter(|m| matches!(m, Message::Key(_, _)))
                    .count(),
                400
            );
            assert!(messages.iter().filter(|m| **m == Message::Refresh).count() >= 2);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "queued input did not reach the server"
        );
        drop(messages);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(matches!(
        received.recv_timeout(Duration::from_secs(1)).unwrap(),
        Event::Frame
    ));
    let started = Instant::now();
    drop(connection);
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[cfg(target_os = "linux")]
#[path = "gui_x11_tests.rs"]
mod x11;
