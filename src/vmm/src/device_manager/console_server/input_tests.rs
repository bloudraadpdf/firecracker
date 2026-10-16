// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use event_manager::SubscriberOps;
use vmm_sys_util::tempdir::TempDir;

use super::*;
use crate::devices::virtio::device::VirtioDevice;
use crate::devices::virtio::input::Input;
use crate::devices::virtio::input::test_utils::{active_input, event_buffers};
use crate::devices::virtio::test_utils::default_mem;
use crate::snapshot::Persist;
use crate::vstate::memory::{Bytes, GuestAddress, GuestMemoryMmap};

fn server(input: Arc<Mutex<Input>>) -> (ConsoleServer, TempDir) {
    let directory = TempDir::new().unwrap();
    let server = ConsoleServer::new(
        &directory.as_path().join("input.sock"),
        Arc::new(Mutex::new(Vga::default())),
        ConsoleInput::Virtio(input),
        #[cfg(target_arch = "x86_64")]
        None,
    )
    .unwrap();
    (server, directory)
}

fn event(mem: &GuestMemoryMmap, index: u16) -> (u16, u16, i32) {
    let mut bytes = [0; 8];
    mem.read_slice(&mut bytes, GuestAddress(0x8000 + u64::from(index) * 16))
        .unwrap();
    (
        u16::from_le_bytes(bytes[..2].try_into().unwrap()),
        u16::from_le_bytes(bytes[2..4].try_into().unwrap()),
        i32::from_le_bytes(bytes[4..].try_into().unwrap()),
    )
}

#[test]
fn fcc1_set2_keys_reach_the_guest_with_distinct_modifiers_keypad_and_pulses() {
    let mem = default_mem();
    let (device, queues) = active_input(&mem, 8);
    let input = Arc::new(Mutex::new(device));
    let (mut server, _directory) = server(input.clone());
    for (code, extended, evdev) in [
        (0x1c, 0, 30),
        (0x5a, 0, 28),
        (0x5a, 1, 96),
        (0x4a, 1, 98),
        (0x14, 0, 29),
        (0x14, 1, 97),
        (0x11, 0, 56),
        (0x11, 1, 100),
        (0x1f, 1, 125),
        (0x27, 1, 126),
        (0x2f, 1, 127),
        (0xe0, 0, 99),
    ] {
        for pressed in [1, 0] {
            event_buffers(&queues[0], 2, 0x8000);
            assert_eq!(
                server.handle(&request(1, &[code, extended, pressed])),
                ConsoleServer::acknowledge(0)
            );
            assert_eq!(event(&mem, 0), (1, evdev, i32::from(pressed)));
            assert_eq!(event(&mem, 1), (0, 0, 0));
        }
    }
    event_buffers(&queues[0], 4, 0x8000);
    assert_eq!(
        server.handle(&request(1, &[0xe1, 0, 1])),
        ConsoleServer::acknowledge(0)
    );
    assert_eq!(
        (0..4).map(|i| event(&mem, i)).collect::<Vec<_>>(),
        [(1, 119, 1), (0, 0, 0), (1, 119, 0), (0, 0, 0)]
    );
    let used = queues[0].used.idx.get();
    assert_eq!(
        server.handle(&request(1, &[0xe1, 0, 0])),
        ConsoleServer::acknowledge(0)
    );
    assert_eq!(queues[0].used.idx.get(), used);
    assert_eq!(input.lock().unwrap().pending_frames(), 0);
}

#[test]
fn full_virtio_queue_rejects_whole_wire_updates_and_disconnect_preserves_releases() {
    let input = Arc::new(Mutex::new(Input::new().unwrap()));
    let (mut server, _directory) = server(input.clone());
    for _ in 0..64 {
        assert_eq!(
            server.handle(&request(1, &[0x1f, 1, 1])),
            ConsoleServer::acknowledge(0)
        );
    }
    let before = input.lock().unwrap().save();
    for message in [
        request(1, &[0xe1, 0, 1]),
        request(1, &[0x58, 0, 1]),
        request(6, &[1, 1, 0, 2, 0, 0]),
    ] {
        assert_eq!(server.handle(&message), ConsoleServer::acknowledge(2));
        assert_eq!(input.lock().unwrap().save(), before);
    }
    assert_eq!(
        server.handle(&request(3, &[])),
        ConsoleServer::acknowledge(0)
    );
    assert_eq!(input.lock().unwrap().pending_frames(), 1);
    assert_eq!(server.handle(&request(5, &[]))[8..11], [0, 0, 0]);
}

#[test]
fn virtio_disconnect_progresses_without_kick_after_pointer_and_windows_key() {
    use std::time::Instant;

    let mem = default_mem();
    let (device, queues) = active_input(&mem, 8);
    let input = Arc::new(Mutex::new(device));
    let (mut server, _directory) = server(input.clone());
    event_buffers(&queues[0], 2, 0x8000);
    server.handle(&request(1, &[0x1f, 1, 1]));
    event_buffers(&queues[0], 4, 0x8000);
    assert_eq!(
        server.handle(&request(6, &[1, 0xff, 0x7f, 123, 0, 0])),
        ConsoleServer::acknowledge(0)
    );
    assert_eq!(event(&mem, 0), (3, 0, 32767));
    assert_eq!(event(&mem, 1), (3, 1, 123));
    assert_eq!(event(&mem, 2), (1, 272, 1));
    assert_eq!(
        server.handle(&request(3, &[])),
        ConsoleServer::acknowledge(0)
    );
    let mut events = crate::EventManager::new().unwrap();
    events.add_subscriber(input.clone());
    events.run_with_timeout(0).unwrap();
    event_buffers(&queues[0], 5, 0x8000);
    let deadline = Instant::now() + Duration::from_millis(250);
    while input.lock().unwrap().pending_frames() != 0 && Instant::now() < deadline {
        events.run_with_timeout(20).unwrap();
    }
    assert_eq!(input.lock().unwrap().pending_frames(), 0);
    assert_eq!(event(&mem, 2), (1, 125, 0));
    assert_eq!(event(&mem, 3), (1, 272, 0));
    assert_eq!(event(&mem, 4), (0, 0, 0));
}

#[test]
fn virtio_led_feedback_uses_ps2_wire_order_and_partial_guest_acknowledgements() {
    let mem = default_mem();
    let (device, queues) = active_input(&mem, 8);
    let input = Arc::new(Mutex::new(device));
    let (mut server, _directory) = server(input.clone());
    event_buffers(&queues[0], 6, 0x8000);
    server.handle(&request(1, &[0x58, 0, 1]));
    server.handle(&request(1, &[0x58, 0, 1]));
    server.handle(&request(1, &[0x77, 0, 1]));
    assert_eq!(server.handle(&request(5, &[]))[8..11], [1, 0, 6]);
    let mut events = crate::EventManager::new().unwrap();
    events.add_subscriber(input.clone());
    for (index, led, expected, pending) in [(0u16, 1u16, 4, 2), (1, 0, 6, 0), (2, 2, 7, 0)] {
        let mut bytes = [0; 8];
        bytes[..2].copy_from_slice(&17u16.to_le_bytes());
        bytes[2..4].copy_from_slice(&led.to_le_bytes());
        bytes[4..].copy_from_slice(&1i32.to_le_bytes());
        let address = 0x9000 + u64::from(index) * 16;
        mem.write_slice(&bytes, GuestAddress(address)).unwrap();
        queues[1].dtable[usize::from(index)].set(address, 8, 0, 0);
        queues[1].avail.ring[usize::from(index)].set(index);
        queues[1].avail.idx.set(index + 1);
        input.lock().unwrap().queue_events()[1].write(1).unwrap();
        events.run_with_timeout(0).unwrap();
        assert_eq!(
            server.handle(&request(5, &[]))[8..11],
            [1, expected, pending]
        );
    }
}

#[test]
fn selected_input_and_console_subscriptions_follow_the_startup_result() {
    use super::super::platform::PlatformRegistration;

    for commit in [false, true] {
        let input: Arc<Mutex<dyn VirtioDevice>> = Arc::new(Mutex::new(Input::new().unwrap()));
        let directory = TempDir::new().unwrap();
        let server = Arc::new(Mutex::new(
            ConsoleServer::new(
                &directory.as_path().join("guard.sock"),
                Arc::new(Mutex::new(Vga::default())),
                ConsoleInput::Virtio(input.clone()),
                #[cfg(target_arch = "x86_64")]
                None,
            )
            .unwrap(),
        ));
        let weak_input = Arc::downgrade(&input);
        let weak_server = Arc::downgrade(&server);
        let mut events = crate::EventManager::new().unwrap();
        let startup = |events: &mut crate::EventManager| -> io::Result<()> {
            let registration =
                PlatformRegistration::new(events, Some(&server))?.with_input(Some(&input))?;
            if commit {
                registration.commit();
                Ok(())
            } else {
                Err(io::Error::other("injected vCPU startup failure"))
            }
        };
        if commit {
            startup(&mut events).unwrap();
        } else {
            startup(&mut events).unwrap_err();
            input
                .lock()
                .unwrap()
                .as_any()
                .downcast_ref::<Input>()
                .unwrap()
                .check_registered()
                .unwrap_err();
        }
        drop(server);
        drop(input);
        assert_eq!(weak_server.upgrade().is_some(), commit);
        assert_eq!(weak_input.upgrade().is_some(), commit);
        drop(events);
        assert!(weak_server.upgrade().is_none());
        assert!(weak_input.upgrade().is_none());
    }
}

#[test]
fn input_registration_failure_rolls_back_the_console_and_partial_watches() {
    use super::super::platform::PlatformRegistration;

    let input = Arc::new(Mutex::new(Input::new().unwrap()));
    let (server, _directory) = server(input.clone());
    let server = Arc::new(Mutex::new(server));
    let mut events = crate::EventManager::new().unwrap();
    let blocker = events.add_subscriber(Arc::new(Mutex::new(Input::new().unwrap())));
    let conflict = Events::new(&input.lock().unwrap().queue_events()[1], EventSet::IN);
    events.event_ops(blocker).unwrap().add(conflict).unwrap();
    let input: Arc<Mutex<dyn VirtioDevice>> = input;
    let registration = PlatformRegistration::new(&mut events, Some(&server))
        .unwrap()
        .with_input(Some(&input));
    assert!(registration.is_err());
    drop(registration);
    input
        .lock()
        .unwrap()
        .as_any()
        .downcast_ref::<Input>()
        .unwrap()
        .check_registered()
        .unwrap_err();
    events.event_ops(blocker).unwrap().remove(conflict).unwrap();
    let retry = PlatformRegistration::new(&mut events, Some(&server))
        .unwrap()
        .with_input(Some(&input))
        .unwrap();
    drop(retry);
    let weak_server = Arc::downgrade(&server);
    let weak_input = Arc::downgrade(&input);
    drop(server);
    drop(input);
    assert!(weak_server.upgrade().is_none());
    assert!(weak_input.upgrade().is_none());
}
