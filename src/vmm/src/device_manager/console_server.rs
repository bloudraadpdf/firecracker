// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Bounded local display/input protocol; remote sessions belong to the sidecar.

use std::collections::{BTreeSet, HashMap};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use event_manager::{EventOps, Events, MutEventSubscriber};
use utils::time::TimerFd;
use vmm_sys_util::epoll::EventSet;

use super::console_optical::OpticalDevice;
use crate::devices::display::Vga;
use crate::devices::legacy::I8042Device;

const CLIENT_LIMIT: usize = 4;
const REQUEST_SIZE: usize = 12;
const CLIENT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug)]
struct Client {
    stream: UnixStream,
    request: [u8; REQUEST_SIZE],
    received: usize,
    response: Vec<u8>,
    sent: usize,
    deadline: Instant,
}

#[derive(Debug)]
struct SocketPath {
    path: PathBuf,
    device: u64,
    inode: u64,
}

impl SocketPath {
    fn capture(path: &Path) -> io::Result<Self> {
        let path = std::env::current_dir()?.join(path);
        let metadata = std::fs::symlink_metadata(&path)?;
        if !metadata.file_type().is_socket() {
            return Err(io::Error::other("console path is not a socket"));
        }
        Ok(Self {
            path,
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
}

impl Drop for SocketPath {
    fn drop(&mut self) {
        if std::fs::symlink_metadata(&self.path).is_ok_and(|metadata| {
            metadata.file_type().is_socket()
                && metadata.dev() == self.device
                && metadata.ino() == self.inode
        }) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[derive(Debug)]
pub struct ConsoleServer {
    listener: UnixListener,
    _socket_path: SocketPath,
    registration: Result<(), String>,
    timer: TimerFd,
    clients: HashMap<RawFd, Client>,
    vga: Arc<Mutex<Vga>>,
    input: Arc<Mutex<I8042Device>>,
    optical: Option<Arc<Mutex<OpticalDevice>>>,
    held: BTreeSet<(u8, bool)>,
    release_pending: bool,
    tablet: Option<Arc<Mutex<super::console_usb::UsbDevice>>>,
}

impl ConsoleServer {
    pub fn new(
        path: &Path,
        vga: Arc<Mutex<Vga>>,
        input: Arc<Mutex<I8042Device>>,
        optical: Option<Arc<Mutex<OpticalDevice>>>,
    ) -> io::Result<Self> {
        let listener = UnixListener::bind(path)?;
        let socket_path = SocketPath::capture(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let mut timer = TimerFd::new();
        timer.arm(Duration::from_secs(1), Some(Duration::from_secs(1)));
        Ok(Self {
            listener,
            _socket_path: socket_path,
            registration: Err("console event subscriber is not registered".to_owned()),
            timer,
            clients: HashMap::new(),
            vga,
            input,
            optical,
            held: BTreeSet::new(),
            release_pending: false,
            tablet: None,
        })
    }

    /// Check event registration after adding this subscriber to the event manager.
    pub fn check_registered(&self) -> io::Result<()> {
        self.registration
            .as_ref()
            .copied()
            .map_err(|error| io::Error::other(error.clone()))
    }

    fn valid_key(code: u8, extended: bool) -> bool {
        if matches!(code, 0xe0 | 0xe1) {
            !extended
        } else {
            I8042Device::valid_key(code, extended)
        }
    }

    pub fn held_keys(&self) -> Vec<(u8, bool)> {
        self.held.iter().copied().collect()
    }

    pub fn set_tablet(&mut self, tablet: Arc<Mutex<super::console_usb::UsbDevice>>) {
        self.tablet = Some(tablet);
    }

    pub fn release_pending(&self) -> bool {
        self.release_pending
    }

    pub fn restore_release_pending(&mut self, pending: bool) {
        self.release_pending = pending;
        let period = if pending {
            Duration::from_millis(10)
        } else {
            Duration::from_secs(1)
        };
        self.timer.arm(period, Some(period));
    }

    pub fn restore_held(&mut self, held: &[(u8, bool)]) -> io::Result<()> {
        if held.len() > 128
            || held
                .iter()
                .any(|&(code, extended)| code == 0xe1 || !Self::valid_key(code, extended))
        {
            return Err(io::Error::other("invalid held-key snapshot"));
        }
        let keys: BTreeSet<_> = held.iter().copied().collect();
        if keys.len() != held.len() {
            return Err(io::Error::other("duplicate held-key snapshot"));
        }
        self.held = keys;
        Ok(())
    }

    fn acknowledge(status: u32) -> Vec<u8> {
        let mut bytes = vec![0; 16];
        bytes[..4].copy_from_slice(b"FCA1");
        bytes[4..8].copy_from_slice(&status.to_le_bytes());
        bytes
    }

    fn release_keys(&mut self) -> bool {
        let mut input = self.input.lock().unwrap();
        let mut released = Vec::new();
        for &(code, extended) in &self.held {
            let result = if code == 0xe0 {
                input.inject_print_screen(false)
            } else {
                input.inject_key(code, extended, false)
            };
            if result.is_ok() {
                released.push((code, extended));
            }
        }
        for key in released {
            self.held.remove(&key);
        }
        let keys_released = self.held.is_empty();
        drop(input);
        let tablet_released = self
            .tablet
            .as_ref()
            .is_none_or(|tablet| tablet.lock().unwrap().release().is_ok());
        let released = keys_released && tablet_released;
        self.restore_release_pending(!released);
        released
    }

    fn handle(&mut self, request: &[u8; REQUEST_SIZE]) -> Vec<u8> {
        if &request[..4] != b"FCC1" {
            return Self::acknowledge(1);
        }
        if matches!(request[4], 1 | 6) && self.release_pending && !self.release_keys() {
            return Self::acknowledge(2);
        }
        match request[4] {
            0 if request[5..].iter().all(|byte| *byte == 0) => {
                let frame = self.vga.lock().unwrap().render();
                let mut response = Vec::with_capacity(16 + frame.pixels.len() * 4);
                response.extend_from_slice(b"FCF1");
                response.extend_from_slice(&u32::from(frame.width).to_le_bytes());
                response.extend_from_slice(&u32::from(frame.height).to_le_bytes());
                response.extend_from_slice(
                    &u32::try_from(frame.pixels.len() * 4).unwrap().to_le_bytes(),
                );
                for pixel in frame.pixels {
                    response.extend_from_slice(&pixel.to_le_bytes());
                }
                response
            }
            1 if request[6] <= 1
                && request[7] <= 1
                && request[8..].iter().all(|byte| *byte == 0) =>
            {
                let (code, extended, pressed) = (request[5], request[6] != 0, request[7] != 0);
                if !Self::valid_key(code, extended) {
                    return Self::acknowledge(1);
                }
                let mut input = self.input.lock().unwrap();
                let result = match code {
                    0xe1 if pressed => input.inject_pause(),
                    0xe1 => return Self::acknowledge(0),
                    0xe0 => input.inject_print_screen(pressed),
                    _ => input.inject_key(code, extended, pressed),
                };
                if result.is_err() {
                    return Self::acknowledge(2);
                }
                if code != 0xe1 {
                    if pressed {
                        self.held.insert((code, extended));
                    } else {
                        self.held.remove(&(code, extended));
                    }
                }
                Self::acknowledge(0)
            }
            3 if request[5..].iter().all(|byte| *byte == 0) => {
                let released = self.release_keys();
                Self::acknowledge(if released { 0 } else { 2 })
            }
            4 if request[5..].iter().all(|byte| *byte == 0) => {
                let result = self
                    .optical
                    .as_ref()
                    .is_some_and(|device| device.lock().unwrap().eject().is_ok());
                Self::acknowledge(if result { 0 } else { 2 })
            }
            5 if request[5..].iter().all(|byte| *byte == 0) => {
                let mut response = Self::acknowledge(0);
                response[8] = u8::from(
                    self.tablet
                        .as_ref()
                        .is_some_and(|tablet| tablet.lock().unwrap().tablet_ready()),
                );
                let input = self.input.lock().unwrap();
                response[9] = input.keyboard_leds();
                response[10] = input.keyboard_pending_leds();
                response
            }
            6 if request[5] <= 7 && request[10] != 128 && request[11] == 0 => {
                let x = u16::from_le_bytes([request[6], request[7]]);
                let y = u16::from_le_bytes([request[8], request[9]]);
                let accepted = self.tablet.as_ref().is_some_and(|tablet| {
                    tablet
                        .lock()
                        .unwrap()
                        .pointer(x, y, request[5], i8::from_ne_bytes([request[10]]))
                        .is_ok()
                });
                Self::acknowledge(if accepted { 0 } else { 2 })
            }
            _ => Self::acknowledge(1),
        }
    }

    fn accept(&mut self, ops: &mut EventOps) {
        for _ in 0..CLIENT_LIMIT {
            let Ok((stream, _)) = self.listener.accept() else {
                break;
            };
            if self.clients.len() >= CLIENT_LIMIT {
                continue;
            }
            if stream.set_nonblocking(true).is_err() {
                continue;
            }
            let client = Client {
                stream,
                request: [0; REQUEST_SIZE],
                received: 0,
                response: Vec::new(),
                sent: 0,
                deadline: Instant::now() + CLIENT_TIMEOUT,
            };
            if ops.add(Events::new(&client.stream, EventSet::IN)).is_ok() {
                self.clients.insert(client.stream.as_raw_fd(), client);
            }
        }
    }

    fn service(&mut self, fd: RawFd, ops: &mut EventOps) {
        let Some(mut client) = self.clients.remove(&fd) else {
            return;
        };
        let keep = self.transfer(&mut client);
        if keep {
            let interest = if client.response.is_empty() {
                EventSet::IN
            } else {
                EventSet::OUT
            };
            if ops.modify(Events::new(&client.stream, interest)).is_ok() {
                self.clients.insert(fd, client);
                return;
            }
        }
        let _ = ops.remove(Events::new(&client.stream, EventSet::IN));
    }

    fn transfer(&mut self, client: &mut Client) -> bool {
        if client.response.is_empty() {
            match client.stream.read(&mut client.request[client.received..]) {
                Ok(0) => return false,
                Ok(count) => client.received += count,
                Err(error) => {
                    return matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    );
                }
            }
            if client.received != REQUEST_SIZE {
                return true;
            }
            client.response = self.handle(&client.request);
        }
        // Bound each event-loop turn so a console cannot monopolise the VMM.
        let end = (client.sent + 65536).min(client.response.len());
        match client.stream.write(&client.response[client.sent..end]) {
            Ok(0) => return false,
            Ok(count) => client.sent += count,
            Err(error) => {
                return matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                );
            }
        }
        client.sent < client.response.len()
    }
}

impl MutEventSubscriber for ConsoleServer {
    fn init(&mut self, ops: &mut EventOps) {
        let listener = Events::new(&self.listener, EventSet::IN);
        let timer = Events::new(&self.timer, EventSet::IN);
        if let Err(error) = ops.add(listener) {
            self.registration = Err(error.to_string());
            return;
        }
        if let Err(error) = ops.add(timer) {
            let _ = ops.remove(Events::new(&self.listener, EventSet::IN));
            self.registration = Err(error.to_string());
            return;
        }
        self.registration = Ok(());
    }

    fn process(&mut self, event: Events, ops: &mut EventOps) {
        if event.fd() == self.listener.as_raw_fd() {
            self.accept(ops);
        } else if event.fd() == self.timer.as_raw_fd() {
            self.timer.read();
            if self.release_pending {
                self.release_keys();
            }
            let now = Instant::now();
            self.clients.retain(|_, client| {
                if now < client.deadline {
                    return true;
                }
                let _ = ops.remove(Events::new(&client.stream, EventSet::IN));
                false
            });
        } else {
            self.service(event.fd(), ops);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ops::{Deref, DerefMut};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use vmm_sys_util::eventfd::EventFd;

    use super::*;
    use crate::vstate::bus::BusDevice;

    struct Fixture {
        server: ConsoleServer,
    }

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "console-server-{}-{}.sock",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let input = I8042Device::new(EventFd::new(libc::EFD_NONBLOCK).unwrap()).unwrap();
            let server = ConsoleServer::new(
                &path,
                Arc::new(Mutex::new(Vga::default())),
                Arc::new(Mutex::new(input)),
                None,
            )
            .unwrap();
            Self { server }
        }
    }

    impl Deref for Fixture {
        type Target = ConsoleServer;
        fn deref(&self) -> &Self::Target {
            &self.server
        }
    }

    impl DerefMut for Fixture {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.server
        }
    }

    fn request(op: u8, payload: &[u8]) -> [u8; REQUEST_SIZE] {
        let mut request = [0; REQUEST_SIZE];
        request[..4].copy_from_slice(b"FCC1");
        request[4] = op;
        request[5..5 + payload.len()].copy_from_slice(payload);
        request
    }

    fn drain_input(server: &ConsoleServer) -> Vec<u8> {
        let mut input = server.input.lock().unwrap();
        let mut result = Vec::new();
        loop {
            let mut status = [0];
            input.read(0, 4, &mut status);
            if status[0] & 1 == 0 {
                return result;
            }
            input.read(0, 0, &mut status);
            result.push(status[0]);
        }
    }

    #[test]
    fn frame_response_has_bounded_consistent_header_and_pixels() {
        let mut server = Fixture::new();
        let expected = server.vga.lock().unwrap().render();
        let response = server.handle(&request(0, &[]));
        assert_eq!(&response[..4], b"FCF1");
        assert_eq!(
            u32::from_le_bytes(response[4..8].try_into().unwrap()),
            u32::from(expected.width)
        );
        assert_eq!(
            u32::from_le_bytes(response[8..12].try_into().unwrap()),
            u32::from(expected.height)
        );
        assert_eq!(
            u32::from_le_bytes(response[12..16].try_into().unwrap()) as usize,
            response.len() - 16
        );
        assert_eq!(response.len() - 16, expected.pixels.len() * 4);
    }

    #[test]
    fn malformed_input_does_not_change_keys_or_queue() {
        let mut server = Fixture::new();
        for invalid in [
            request(1, &[0x1c, 2, 1]),
            request(1, &[0x1c, 0, 2]),
            request(2, &[]),
            request(2, &[8]),
            request(3, &[1]),
            request(0, &[1]),
            request(6, &[0, 0, 0, 0, 0, 128]),
            request(6, &[0, 0, 0, 0, 0, 0, 1]),
        ] {
            assert_eq!(server.handle(&invalid), ConsoleServer::acknowledge(1));
        }
        assert!(server.held_keys().is_empty());
        assert!(drain_input(&server).is_empty());
        assert_eq!(
            server.handle(&request(4, &[])),
            ConsoleServer::acknowledge(2)
        );
    }

    #[test]
    fn snapshot_restores_held_keys_for_disconnect_release() {
        let mut original = Fixture::new();
        assert_eq!(
            original.handle(&request(1, &[0x14, 0, 1])),
            ConsoleServer::acknowledge(0)
        );
        assert_eq!(
            original.handle(&request(1, &[0x71, 1, 1])),
            ConsoleServer::acknowledge(0)
        );
        assert_eq!(drain_input(&original), [0x14, 0xe0, 0x71]);
        let state = original.input.lock().unwrap().save_state();
        let mut restored = Fixture::new();
        restored
            .input
            .lock()
            .unwrap()
            .restore_state(&state)
            .unwrap();
        restored.restore_held(&original.held_keys()).unwrap();
        assert_eq!(
            restored.handle(&request(3, &[])),
            ConsoleServer::acknowledge(0)
        );
        assert!(restored.held_keys().is_empty());
        assert_eq!(drain_input(&restored), [0xf0, 0x14, 0xe0, 0xf0, 0x71]);
    }

    #[test]
    fn led_query_uses_guest_commands_and_restored_state() {
        let mut original = Fixture::new();
        assert_eq!(original.handle(&request(5, &[]))[9], 0);
        {
            let mut input = original.input.lock().unwrap();
            input.write(0, 0, &[0xed]);
            input.write(0, 0, &[6]);
        }
        assert_eq!(
            original.handle(&request(5, &[]))[8..],
            [0, 6, 0, 0, 0, 0, 0, 0]
        );
        original.handle(&request(1, &[0x58, 0, 1]));
        assert_eq!(original.handle(&request(5, &[]))[10], 4);
        let state = original.input.lock().unwrap().save_state();
        let mut restored = Fixture::new();
        restored
            .input
            .lock()
            .unwrap()
            .restore_state(&state)
            .unwrap();
        assert_eq!(restored.handle(&request(5, &[]))[9], 6);
        assert_eq!(restored.handle(&request(5, &[]))[10], 4);
    }

    #[test]
    fn full_queue_defers_release_and_restores_timer_retry() {
        use event_manager::SubscriberOps;

        let mut original = Fixture::new();
        for _ in 0..256 {
            assert_eq!(
                original.handle(&request(1, &[0x1c, 0, 1])),
                ConsoleServer::acknowledge(0)
            );
        }
        assert_eq!(
            original.handle(&request(3, &[])),
            ConsoleServer::acknowledge(2)
        );
        assert!(original.release_pending());
        assert_eq!(
            original.handle(&request(1, &[0x32, 0, 1])),
            ConsoleServer::acknowledge(2)
        );
        assert_eq!(original.held_keys(), [(0x1c, false)]);
        let state = original.input.lock().unwrap().save_state();
        let mut restored = Fixture::new();
        restored
            .input
            .lock()
            .unwrap()
            .restore_state(&state)
            .unwrap();
        restored.restore_held(&original.held_keys()).unwrap();
        restored.restore_release_pending(original.release_pending());
        assert_eq!(drain_input(&restored), vec![0x1c; 256]);
        let restored = Arc::new(Mutex::new(restored.server));
        let mut events = crate::EventManager::new().unwrap();
        events.add_subscriber(restored.clone());
        events.run_with_timeout(1000).unwrap();
        let mut restored = restored.lock().unwrap();
        assert!(!restored.release_pending());
        assert!(restored.held_keys().is_empty());
        assert_eq!(drain_input(&restored), [0xf0, 0x1c]);
        assert_eq!(
            restored.handle(&request(1, &[0x32, 0, 1])),
            ConsoleServer::acknowledge(0)
        );
        assert_eq!(drain_input(&restored), [0x32]);
    }

    #[test]
    fn fragmented_request_waits_until_complete() {
        let mut server = Fixture::new();
        let (mut peer, stream) = UnixStream::pair().unwrap();
        stream.set_nonblocking(true).unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        let mut client = Client {
            stream,
            request: [0; REQUEST_SIZE],
            received: 0,
            response: Vec::new(),
            sent: 0,
            deadline: Instant::now() + CLIENT_TIMEOUT,
        };
        let request = request(1, &[0x1c, 0, 1]);
        peer.write_all(&request[..5]).unwrap();
        assert!(server.transfer(&mut client));
        assert!(server.held_keys().is_empty());
        peer.write_all(&request[5..]).unwrap();
        assert!(!server.transfer(&mut client));
        let mut response = [0; 16];
        peer.read_exact(&mut response).unwrap();
        assert_eq!(response.as_slice(), ConsoleServer::acknowledge(0));
        assert_eq!(drain_input(&server), [0x1c]);
    }
    #[test]
    fn held_snapshot_rejects_invalid_codes_and_duplicates_atomically() {
        let mut server = Fixture::new();
        server.restore_held(&[(0x14, false)]).unwrap();
        for invalid in [
            vec![(0xff, false)],
            vec![(0xe1, false)],
            vec![(0xe0, true)],
            vec![(0x1c, true)],
            vec![(0x14, false), (0x14, false)],
            vec![(0x14, false); 129],
        ] {
            server.restore_held(&invalid).unwrap_err();
            assert_eq!(server.held_keys(), [(0x14, false)]);
        }
        assert_eq!(
            server.handle(&request(1, &[0x1c, 1, 1])),
            ConsoleServer::acknowledge(1)
        );
    }

    #[test]
    fn socket_cleanup_preserves_replacement_and_removes_owned_socket() {
        let server = Fixture::new();
        let path = server._socket_path.path.clone();
        assert!(path.exists());
        drop(server);
        assert!(!path.exists());
        let server = Fixture::new();
        let path = server._socket_path.path.clone();
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, b"replacement").unwrap();
        drop(server);
        assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
        std::fs::remove_file(&path).unwrap();
        let server = Fixture::new();
        let path = server._socket_path.path.clone();
        std::fs::remove_file(&path).unwrap();
        let replacement = UnixListener::bind(&path).unwrap();
        drop(server);
        assert!(path.exists());
        drop(replacement);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn failed_boot_rolls_back_subscription_and_allows_socket_reuse() {
        use super::super::platform::PlatformRegistration;
        let mut events = crate::EventManager::new().unwrap();
        let fixture = Fixture::new();
        let path = fixture._socket_path.path.clone();
        let server = Arc::new(Mutex::new(fixture.server));
        let weak = Arc::downgrade(&server);
        let fail_boot = |events: &mut crate::EventManager| -> io::Result<()> {
            let _registration = PlatformRegistration::new(events, Some(&server))?;
            assert_eq!(Arc::strong_count(&server), 2);
            Err(io::Error::other("injected vCPU startup failure"))
        };
        fail_boot(&mut events).unwrap_err();
        assert_eq!(Arc::strong_count(&server), 1);
        drop(server);
        assert!(weak.upgrade().is_none());
        assert!(!path.exists());
        let replacement = UnixListener::bind(&path).unwrap();
        drop(replacement);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn failed_restore_before_registration_releases_socket() {
        let mut fixture = Fixture::new();
        let path = fixture._socket_path.path.clone();
        fixture.restore_held(&[(0xff, false)]).unwrap_err();
        drop(fixture);
        let replacement = UnixListener::bind(&path).unwrap();
        drop(replacement);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn successful_boot_commits_console_subscription() {
        use super::super::platform::PlatformRegistration;
        let mut events = crate::EventManager::new().unwrap();
        let fixture = Fixture::new();
        let path = fixture._socket_path.path.clone();
        let server = Arc::new(Mutex::new(fixture.server));
        let weak = Arc::downgrade(&server);
        PlatformRegistration::new(&mut events, Some(&server))
            .unwrap()
            .commit();
        drop(server);
        assert!(weak.upgrade().is_some());
        assert!(path.exists());
        drop(events);
        assert!(weak.upgrade().is_none());
        assert!(!path.exists());
    }

    #[test]
    fn registration_status_is_observable() {
        use event_manager::SubscriberOps;
        let fixture = Fixture::new();
        fixture.check_registered().unwrap_err();
        let server = Arc::new(Mutex::new(fixture.server));
        let mut manager = crate::EventManager::new().unwrap();
        let id = manager.add_subscriber(server.clone());
        server.lock().unwrap().check_registered().unwrap();
        manager.remove_subscriber(id).unwrap();
    }
}
