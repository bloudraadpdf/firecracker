// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use super::protocol::*;
use super::test_utils::{activate_input, active_input, configure_input, event_buffers, virtqueues};
use super::{Input, InputError};
use crate::devices::virtio::device::VirtioDevice;
use crate::devices::virtio::queue::{VIRTQ_DESC_F_NEXT, VIRTQ_DESC_F_WRITE};
use crate::devices::virtio::test_utils::{VirtQueue, default_mem};
use crate::devices::virtio::transport::mmio::IrqTrigger;
use crate::snapshot::Persist;
use crate::vstate::memory::{Bytes, GuestAddress, GuestMemoryMmap};
use event_manager::SubscriberOps;
use std::sync::{Arc, Mutex};

fn registered_input(device: Input) -> (Arc<Mutex<Input>>, crate::EventManager) {
    let device = Arc::new(Mutex::new(device));
    let mut events = crate::EventManager::new().unwrap();
    events.add_subscriber(device.clone());
    device.lock().unwrap().check_registered().unwrap();
    (device, events)
}

fn input_with_key_down(mem: &GuestMemoryMmap, key: u16) -> (Input, [VirtQueue<'_>; 2]) {
    let (mut device, queues) = active_input(mem, 8);
    event_buffers(&queues[EVENT_QUEUE], 2, 0x8000);
    device.key(key, true).unwrap();
    (device, queues)
}

fn restore_active_input(device: Input, mem: &GuestMemoryMmap) -> Input {
    use super::persist::InputConstructorArgs;

    let state = bitcode::deserialize(&bitcode::serialize(&device.save()).unwrap()).unwrap();
    drop(device);
    let mut device = Input::restore(InputConstructorArgs { mem: mem.clone() }, &state).unwrap();
    device
        .activate(mem.clone(), Arc::new(IrqTrigger::new()))
        .unwrap();
    device
}

fn offer(queue: &VirtQueue<'_>, index: u16, address: u64, length: u32, flags: u16) {
    queue.dtable[usize::from(index)].set(address, length, flags, 0);
    publish(queue, index);
}

fn status_buffer(
    mem: &GuestMemoryMmap,
    queue: &VirtQueue<'_>,
    index: u16,
    event: InputEvent,
    length: u32,
    flags: u16,
) {
    let address = 0x9000 + u64::from(index) * 16;
    mem.write_slice(&event.bytes(), GuestAddress(address))
        .unwrap();
    offer(queue, index, address, length, flags);
}

fn publish(queue: &VirtQueue<'_>, index: u16) {
    let next = queue.avail.idx.get();
    queue.avail.ring[usize::from(next % queue.size())].set(index);
    queue.avail.idx.set(next.wrapping_add(1));
}

fn read_event(mem: &GuestMemoryMmap, address: u64) -> InputEvent {
    let mut bytes = [0; EVENT_SIZE];
    mem.read_slice(&mut bytes, GuestAddress(address)).unwrap();
    InputEvent::from_bytes(bytes)
}

#[test]
fn combined_capabilities_include_windows_modifiers_and_absolute_pointer() {
    let mut device = Input::new().unwrap();
    device.write_config(0, &[0x11, 1]);
    let config = device.config_as_bytes();
    assert_eq!(config[2], 35);
    for key in [
        29u16,
        42,
        54,
        56,
        97,
        100,
        125,
        126,
        BTN_LEFT,
        BTN_LEFT + 1,
        BTN_LEFT + 2,
    ] {
        assert_ne!(config[8 + usize::from(key / 8)] & (1 << (key % 8)), 0);
    }
    device.write_config(1, &[3]);
    assert_eq!(&device.config_as_bytes()[2..9], &[1, 0, 0, 0, 0, 0, 3]);
    for axis in [0, 1] {
        device.write_config(0, &[0x12, axis]);
        let config = device.config_as_bytes();
        assert_eq!(config[2], 20);
        assert_eq!(
            u32::from_le_bytes(config[12..16].try_into().unwrap()),
            u32::from(ABS_MAX)
        );
    }
}

#[test]
fn selected_config_is_bounded_and_only_selectors_are_writable() {
    let mut device = Input::new().unwrap();
    device.write_config(0, &[0x11, 1]);
    let expected = device.config_as_bytes().to_vec();
    device.write_config(2, &[0xff; CONFIG_SIZE]);
    device.write_config(u64::MAX, &[1, 2]);
    assert_eq!(device.config_as_bytes(), expected);
    for select in 0..=u8::MAX {
        for subsel in 0..=u8::MAX {
            device.write_config(0, &[select, subsel]);
            let bytes = device.config_as_bytes();
            assert!(usize::from(bytes[2]) <= 128);
            assert!(
                bytes[8 + usize::from(bytes[2])..]
                    .iter()
                    .all(|&byte| byte == 0)
            );
        }
    }
    let mut bytes = [0xff; 10];
    device.read_config(u64::MAX, &mut bytes);
    assert_eq!(bytes, [0; 10]);
}

#[test]
fn host_input_bounds_do_not_change_held_state() {
    let mut device = Input::new().unwrap();
    for code in [0, 128, 256, BTN_LEFT, u16::MAX] {
        assert!(matches!(
            device.key(code, true),
            Err(InputError::InvalidHostEvent)
        ));
    }
    assert!(matches!(
        device.pointer(ABS_MAX + 1, 0, 0, 0),
        Err(InputError::InvalidHostEvent)
    ));
    assert!(matches!(
        device.pointer(0, 0, 8, 0),
        Err(InputError::InvalidHostEvent)
    ));
    assert!(matches!(
        device.pointer(0, 0, 0, -128),
        Err(InputError::InvalidHostEvent)
    ));
    assert!(device.held.is_empty());
    assert!(device.pending.is_empty());
}

#[test]
fn pause_pulse_admits_two_reports_atomically_and_never_leaves_a_key_held() {
    let mut blocked = Input::new().unwrap();
    for _ in 0..MAX_PENDING_FRAMES - 1 {
        blocked.key(30, true).unwrap();
    }
    let before = blocked.save();
    assert!(matches!(blocked.pulse(119), Err(InputError::PendingFull)));
    assert_eq!(blocked.save(), before);
    assert!(!blocked.retry_timer.is_armed());

    let mem = default_mem();
    let (mut device, queues) = active_input(&mem, 8);
    event_buffers(&queues[EVENT_QUEUE], 4, 0x8000);
    device.pulse(119).unwrap();
    let actual: Vec<_> = (0..4).map(|i| read_event(&mem, 0x8000 + i * 16)).collect();
    assert_eq!(
        actual,
        [
            InputEvent::new(EV_KEY, 119, 1),
            InputEvent::SYN,
            InputEvent::new(EV_KEY, 119, 0),
            InputEvent::SYN,
        ]
    );
    assert_eq!(device.pending_frames(), 0);
    assert!(device.held.is_empty() && device.delivered.is_empty());
    assert!(!device.retry_timer.is_armed());
    device.key(119, true).unwrap();
    assert!(matches!(
        device.pulse(119),
        Err(InputError::InvalidHostEvent)
    ));
}

#[test]
fn modern_feature_negotiation_is_required_and_masks_unknown_bits() {
    let mem = default_mem();
    let (mut device, _) = active_input(&mem, 8);
    device.reset();
    assert!(matches!(
        device.activate(mem.clone(), Arc::new(IrqTrigger::new())),
        Err(crate::devices::virtio::ActivateError::RequiredFeatureNotAcked(_))
    ));
    device.ack_features_by_page(0, u32::MAX);
    device.ack_features_by_page(2, u32::MAX);
    assert_eq!(device.acked_features(), 0);
    device.ack_features_by_page(1, u32::MAX);
    assert_eq!(device.acked_features(), super::device::FEATURES);
    device.set_acked_features(u64::MAX);
    assert_eq!(device.acked_features(), device.avail_features());
}

#[test]
#[should_panic(expected = "Input device is already activated")]
fn activation_cannot_rebind_live_queue_memory() {
    let mem = default_mem();
    let (mut device, _queues) = active_input(&mem, 8);
    let remapped = default_mem();
    device
        .activate(remapped, Arc::new(IrqTrigger::new()))
        .unwrap();
}

#[test]
fn complete_pointer_update_survives_descriptor_backpressure() {
    let mem = default_mem();
    let (mut device, queues) = active_input(&mem, 8);
    event_buffers(&queues[EVENT_QUEUE], 3, 0x8000);
    device.pointer(ABS_MAX, 123, 7, -1).unwrap();
    assert_eq!(queues[EVENT_QUEUE].used.idx.get(), 3);
    assert_eq!(device.pending.front().unwrap().next, 3);
    assert_eq!(
        device.pending.front().unwrap().events.last(),
        Some(&InputEvent::SYN)
    );
    for index in 3..7 {
        offer(
            &queues[EVENT_QUEUE],
            index,
            0x8000 + u64::from(index) * 16,
            8,
            VIRTQ_DESC_F_WRITE,
        );
    }
    let (device, mut events) = registered_input(device);
    events.run_with_timeout(0).unwrap();
    assert!(device.lock().unwrap().pending.is_empty());
    let actual: Vec<_> = (0..7).map(|i| read_event(&mem, 0x8000 + i * 16)).collect();
    assert_eq!(
        actual,
        [
            InputEvent::new(EV_ABS, 0, i32::from(ABS_MAX)),
            InputEvent::new(EV_ABS, 1, 123),
            InputEvent::new(EV_KEY, BTN_LEFT, 1),
            InputEvent::new(EV_KEY, BTN_LEFT + 1, 1),
            InputEvent::new(EV_KEY, BTN_LEFT + 2, 1),
            InputEvent::new(EV_REL, 8, -1),
            InputEvent::SYN,
        ]
    );
    assert_eq!(queues[EVENT_QUEUE].used.idx.get(), 7);
    for element in &queues[EVENT_QUEUE].used.ring[..7] {
        assert_eq!(element.get().len, 8);
    }
}

#[test]
fn firmware_buffer_replenishment_without_kick_completes_disconnect_release() {
    use std::time::{Duration, Instant};

    let mem = default_mem();
    let (device, queues) = input_with_key_down(&mem, 125);
    let (device, mut events) = registered_input(device);
    events.run_with_timeout(0).unwrap();
    {
        let mut device = device.lock().unwrap();
        assert!(!device.retry_armed);
        device.release_all().unwrap();
        assert!(device.retry_armed);
    }
    event_buffers(&queues[EVENT_QUEUE], 4, 0x9000);
    let deadline = Instant::now() + Duration::from_millis(250);
    while !device.lock().unwrap().pending.is_empty() && Instant::now() < deadline {
        events.run_with_timeout(20).unwrap();
    }
    let device = device.lock().unwrap();
    assert!(
        device.pending.is_empty(),
        "firmware requeued buffers without a notification"
    );
    assert!(device.delivered.is_empty());
    assert!(!device.retry_armed);
    assert!(!device.retry_timer.is_armed());
    assert_eq!(read_event(&mem, 0x9020), InputEvent::new(EV_KEY, 125, 0));
    assert_eq!(read_event(&mem, 0x9030), InputEvent::SYN);
}

#[test]
fn event_and_status_scatter_buffers_preserve_wire_order_and_dirty_pages() {
    use crate::vmm_config::machine_config::HugePageConfig;
    use crate::vstate::memory::{GuestMemoryExtension, anonymous, test_utils::into_region_ext};
    use vm_memory::{GuestMemoryBackend, GuestMemoryRegion, bitmap::Bitmap};

    let mem = into_region_ext(
        anonymous(
            [(GuestAddress(0), 0x10000)].into_iter(),
            true,
            HugePageConfig::None,
        )
        .unwrap(),
    );
    let (mut device, queues) = active_input(&mem, 8);
    queues[EVENT_QUEUE].dtable[0].set(0x8000, 3, VIRTQ_DESC_F_WRITE | VIRTQ_DESC_F_NEXT, 1);
    queues[EVENT_QUEUE].dtable[1].set(0x9000, 5, VIRTQ_DESC_F_WRITE, 0);
    publish(&queues[EVENT_QUEUE], 0);
    let region = mem.iter().next().unwrap();
    mem.reset_dirty();
    device.key(125, true).unwrap();
    assert!(region.bitmap().dirty_at(0x8000));
    assert!(region.bitmap().dirty_at(0x9000));
    let mut bytes = [0; 8];
    mem.read_slice(&mut bytes[..3], GuestAddress(0x8000))
        .unwrap();
    mem.read_slice(&mut bytes[3..], GuestAddress(0x9000))
        .unwrap();
    assert_eq!(
        InputEvent::from_bytes(bytes),
        InputEvent::new(EV_KEY, 125, 1)
    );
    assert_eq!(device.delivered.iter().copied().collect::<Vec<_>>(), [125]);
    let led = InputEvent::new(EV_LED, 1, 1).bytes();
    mem.write_slice(&led[..4], GuestAddress(0xa000)).unwrap();
    mem.write_slice(&led[4..], GuestAddress(0xb000)).unwrap();
    queues[STATUS_QUEUE].dtable[0].set(0xa000, 4, VIRTQ_DESC_F_NEXT, 1);
    queues[STATUS_QUEUE].dtable[1].set(0xb000, 4, 0, 0);
    publish(&queues[STATUS_QUEUE], 0);
    device.process_status().unwrap();
    assert_eq!(device.leds, 2);
    assert_eq!(queues[STATUS_QUEUE].used.ring[0].get().len, 0);
}

#[test]
fn malformed_guest_event_buffers_do_not_consume_pending_input() {
    for (address, length, flags) in [
        (0x8000, 7, VIRTQ_DESC_F_WRITE),
        (u64::MAX, 8, VIRTQ_DESC_F_WRITE),
        (0x8000, 8, 0),
        (0x8000, 8, VIRTQ_DESC_F_WRITE | VIRTQ_DESC_F_NEXT),
    ] {
        let mem = default_mem();
        let (mut device, queues) = active_input(&mem, 8);
        offer(&queues[EVENT_QUEUE], 0, address, length, flags);
        device.key(30, true).unwrap();
        assert_eq!(queues[EVENT_QUEUE].used.idx.get(), 1);
        assert_eq!(queues[EVENT_QUEUE].used.ring[0].get().len, 0);
        assert_eq!(device.pending.front().unwrap().next, 0);
        assert!(device.delivered.is_empty());
        offer(&queues[EVENT_QUEUE], 1, 0x9000, 8, VIRTQ_DESC_F_WRITE);
        device.flush_events().unwrap();
        assert_eq!(read_event(&mem, 0x9000), InputEvent::new(EV_KEY, 30, 1));
    }
}

#[test]
fn indirect_descriptors_are_bounded_and_malformed_chains_do_not_write() {
    use crate::devices::virtio::queue::{Descriptor, VIRTQ_DESC_F_INDIRECT};

    for malformed in [false, true] {
        let mem = default_mem();
        let (mut device, queues) = active_input(&mem, 8);
        let event = InputEvent::new(EV_KEY, 30, 1);
        mem.write_slice(&[0xa5; EVENT_SIZE], GuestAddress(0x9000))
            .unwrap();
        mem.write_obj(
            Descriptor {
                addr: 0x9000,
                len: 8,
                flags: VIRTQ_DESC_F_WRITE | if malformed { VIRTQ_DESC_F_NEXT } else { 0 },
                next: 0,
            },
            GuestAddress(0x8000),
        )
        .unwrap();
        offer(&queues[EVENT_QUEUE], 0, 0x8000, 16, VIRTQ_DESC_F_INDIRECT);
        device.key(30, true).unwrap();
        if malformed {
            let mut bytes = [0; EVENT_SIZE];
            mem.read_slice(&mut bytes, GuestAddress(0x9000)).unwrap();
            assert_eq!(bytes, [0xa5; EVENT_SIZE]);
            assert_eq!(queues[EVENT_QUEUE].used.ring[0].get().len, 0);
        } else {
            assert_eq!(read_event(&mem, 0x9000), event);
            assert_eq!(queues[EVENT_QUEUE].used.ring[0].get().len, 8);
        }
    }
}

#[test]
fn partial_registration_rolls_back_watches_and_can_be_retried() {
    use event_manager::{EventSet, Events};

    for conflict_index in [STATUS_QUEUE, NUM_QUEUES] {
        let mem = default_mem();
        let (mut device, _queues) = active_input(&mem, 8);
        device.key(30, true).unwrap();
        assert!(device.retry_armed);
        let input = Arc::new(Mutex::new(device));
        let mut events = crate::EventManager::new().unwrap();
        let blocker = events.add_subscriber(Arc::new(Mutex::new(Input::new().unwrap())));
        let conflict = {
            let input = input.lock().unwrap();
            if conflict_index == NUM_QUEUES {
                Events::new(&input.retry_timer, EventSet::IN)
            } else {
                Events::new(&input.queue_events[conflict_index], EventSet::IN)
            }
        };
        events.event_ops(blocker).unwrap().add(conflict).unwrap();
        let failed = events.add_subscriber(input.clone());
        {
            let input = input.lock().unwrap();
            input.check_registered().unwrap_err();
            assert!(!input.retry_armed);
            assert!(!input.retry_timer.is_armed());
        }
        events.remove_subscriber(failed).unwrap();
        events.event_ops(blocker).unwrap().remove(conflict).unwrap();
        let subscriber = events.add_subscriber(input.clone());
        {
            let input = input.lock().unwrap();
            input.check_registered().unwrap();
            assert!(input.retry_armed);
        }
        let owner = Arc::downgrade(&input);
        events.remove_subscriber(subscriber).unwrap();
        drop(input);
        assert!(owner.upgrade().is_none());
        events.remove_subscriber(blocker).unwrap();
    }
}

#[test]
fn status_feedback_ignores_unsupported_codes_values_and_bad_directions() {
    let mem = default_mem();
    let (mut device, queues) = active_input(&mem, 8);
    for (index, (event, length, flags)) in [
        (InputEvent::new(EV_LED, 0, 1), 8, 0),
        (InputEvent::new(EV_LED, 2, 1), 8, 0),
        (InputEvent::new(EV_LED, u16::MAX, 1), 8, 0),
        (InputEvent::new(EV_LED, 0, -1), 8, 0),
        (InputEvent::new(EV_KEY, 30, 1), 8, 0),
        (InputEvent::new(EV_LED, 0, 0), 7, 0),
        (InputEvent::new(EV_LED, 0, 0), 8, VIRTQ_DESC_F_WRITE),
    ]
    .into_iter()
    .enumerate()
    {
        status_buffer(
            &mem,
            &queues[STATUS_QUEUE],
            index.try_into().unwrap(),
            event,
            length,
            flags,
        );
    }
    device.process_status().unwrap();
    assert_eq!(device.leds, 5);
    assert_eq!(queues[STATUS_QUEUE].used.idx.get(), 7);
    assert!(device.held.is_empty());
}

#[test]
fn lock_feedback_survives_repeats_partial_guest_acknowledgements_and_restore() {
    let mem = default_mem();
    let (mut device, queues) = active_input(&mem, 8);
    event_buffers(&queues[EVENT_QUEUE], 6, 0x8000);
    device.key(58, true).unwrap();
    device.key(58, true).unwrap();
    device.key(69, true).unwrap();
    assert_eq!(device.led_state(), 0);
    assert_eq!(device.pending_led_state(), 3);
    let mut device = restore_active_input(device, &mem);
    for (index, (event, expected_leds, pending)) in [
        (InputEvent::new(EV_LED, 1, 1), 2, 1),
        (InputEvent::new(EV_LED, 0, -1), 2, 1),
        (InputEvent::new(EV_LED, 0, 1), 3, 0),
    ]
    .into_iter()
    .enumerate()
    {
        status_buffer(
            &mem,
            &queues[STATUS_QUEUE],
            index.try_into().unwrap(),
            event,
            8,
            0,
        );
        device.process_status().unwrap();
        assert_eq!(device.led_state(), expected_leds);
        assert_eq!(device.pending_led_state(), pending);
    }
    device.key(58, false).unwrap();
    device.key(58, true).unwrap();
    assert_eq!(device.pending_led_state(), 2);
    assert!(device.reset());
    assert_eq!(device.led_state(), 0);
    assert_eq!(device.pending_led_state(), 0);
}

#[test]
fn invalid_available_index_fails_closed_on_both_queues() {
    for index in [EVENT_QUEUE, STATUS_QUEUE] {
        let mem = default_mem();
        let (mut device, queues) = active_input(&mem, 8);
        device.key(30, true).unwrap();
        queues[index].avail.idx.set(9);
        let result = if index == EVENT_QUEUE {
            device.flush_events()
        } else {
            device.process_status()
        };
        assert!(matches!(result, Err(InputError::Avail(_))));
    }
}

#[test]
fn full_host_queue_returns_backpressure_without_losing_disconnect_releases() {
    let mem = default_mem();
    let (mut device, queues) = input_with_key_down(&mem, 30);
    device.key(30, false).unwrap();
    for _ in 1..MAX_PENDING_FRAMES {
        device.key(32, true).unwrap();
    }
    assert!(matches!(
        device.key(32, false),
        Err(InputError::PendingFull)
    ));
    assert!(device.held.contains(&32));
    assert!(!device.held.contains(&30));
    assert!(device.delivered.contains(&30));
    device.release_all().unwrap();
    assert!(device.held.is_empty());
    assert_eq!(device.pending.len(), 1);
    event_buffers(&queues[EVENT_QUEUE], 5, 0x9000);
    device.flush_events().unwrap();
    assert!(device.pending.is_empty());
    assert!(device.delivered.is_empty());
    assert_eq!(read_event(&mem, 0x9020), InputEvent::new(EV_KEY, 30, 0));
    assert_eq!(read_event(&mem, 0x9030), InputEvent::new(EV_KEY, 32, 0));
    assert_eq!(read_event(&mem, 0x9040), InputEvent::SYN);
}

#[test]
fn reset_clears_held_input_and_existing_watches_support_reactivation() {
    let mem = default_mem();
    let (mut device, _queues) = input_with_key_down(&mem, 126);
    device.key(30, true).unwrap();
    device.write_config(0, &[0x11, 1]);
    device.leds = 7;
    let (device, mut events) = registered_input(device);
    {
        let mut device = device.lock().unwrap();
        assert!(device.retry_armed);
        assert!(device.reset());
        assert!(!device.retry_armed);
        assert!(!device.retry_timer.is_armed());
        assert!(device.held.is_empty() && device.delivered.is_empty() && device.pending.is_empty());
        assert_eq!(device.leds, 0);
        assert_eq!(device.config_as_bytes(), config(0, 0));
        assert_eq!(device.acked_features(), 0);
        assert!(device.queues().iter().all(|queue| !queue.ready));
        device.queue_events()[STATUS_QUEUE].write(1).unwrap();
    }
    events.run_with_timeout(0).unwrap();
    let queues = virtqueues(&mem, 8);
    offer(&queues[EVENT_QUEUE], 0, 0x9000, 8, VIRTQ_DESC_F_WRITE);
    {
        let mut device = device.lock().unwrap();
        activate_input(&mut device, &mem, &queues);
        device.key(125, true).unwrap();
    }
    events.run_with_timeout(0).unwrap();
    assert_eq!(read_event(&mem, 0x9000), InputEvent::new(EV_KEY, 125, 1));
    assert_eq!(queues[EVENT_QUEUE].used.idx.get(), 1);
}

#[test]
fn snapshot_replays_partial_frame_and_held_meta_key_without_duplicate_press() {
    use super::persist::{InputConstructorArgs, InputState};

    let mem = default_mem();
    let (mut device, queues) = active_input(&mem, 8);
    offer(&queues[EVENT_QUEUE], 0, 0x8000, 8, VIRTQ_DESC_F_WRITE);
    device.key(125, true).unwrap();
    device.write_config(0, &[0x12, 1]);
    device.leds = 3;
    let state = device.save();
    assert_eq!(state.pending.front().unwrap().next, 1);
    let bytes = bitcode::serialize(&state).unwrap();
    let restored_state: InputState = bitcode::deserialize(&bytes).unwrap();
    assert_eq!(state, restored_state);
    drop(device);
    let mut device =
        Input::restore(InputConstructorArgs { mem: mem.clone() }, &restored_state).unwrap();
    assert!(!device.is_activated());
    assert!(!device.retry_armed);
    assert!(!device.retry_timer.is_armed());
    assert_eq!(device.led_state(), 3);
    assert_eq!(device.config_as_bytes(), config(0x12, 1));
    offer(&queues[EVENT_QUEUE], 1, 0x9000, 8, VIRTQ_DESC_F_WRITE);
    device
        .activate(mem.clone(), Arc::new(IrqTrigger::new()))
        .unwrap();
    assert!(device.retry_armed);
    let (device, mut events) = registered_input(device);
    events.run_with_timeout(0).unwrap();
    let mut device = device.lock().unwrap();
    assert!(!device.retry_armed);
    assert!(!device.retry_timer.is_armed());
    assert_eq!(read_event(&mem, 0x9000), InputEvent::SYN);
    assert!(device.pending.is_empty());
    assert_eq!(device.held.iter().copied().collect::<Vec<_>>(), [125]);
    device.key(125, false).unwrap();
    assert_eq!(
        device.pending.front().unwrap().events[0],
        InputEvent::new(EV_KEY, 125, 0)
    );
}

#[test]
fn corrupt_input_snapshots_reject_unbounded_or_inconsistent_state() {
    use super::device::Frame;
    use super::persist::{InputConstructorArgs, InputPersistError};

    let mem = default_mem();
    let mut device = Input::new().unwrap();
    device.key(30, true).unwrap();
    let good = device.save();
    Input::restore(InputConstructorArgs { mem: mem.clone() }, &good).unwrap();
    for case in 0..14 {
        let mut state = good.clone();
        match case {
            0 => state.virtio_state.avail_features |= 1,
            1 => state.virtio_state.acked_features |= 1,
            2 => state.position[0] = u16::MAX,
            3 => state.leds = 8,
            4 => {
                state.held.insert(u16::MAX);
            }
            5 => {
                state.delivered.insert(30);
            }
            6 => state.pending.front_mut().unwrap().next = 2,
            7 => state
                .pending
                .front_mut()
                .unwrap()
                .events
                .pop()
                .map(|_| ())
                .unwrap(),
            8 => state.pending.front_mut().unwrap().events[0].value = -1,
            9 => {
                state.pending = std::iter::repeat_n(
                    Frame {
                        events: vec![InputEvent::SYN],
                        next: 0,
                    },
                    MAX_PENDING_FRAMES + 1,
                )
                .collect()
            }
            10 => {
                state.pending.front_mut().unwrap().events =
                    vec![InputEvent::SYN; MAX_FRAME_EVENTS + 1]
            }
            11 => {
                state.held.clear();
            }
            12 => state.expected_leds = 8,
            13 => state.pending_leds = 8,
            _ => unreachable!(),
        }
        assert!(
            matches!(
                Input::restore(InputConstructorArgs { mem: mem.clone() }, &state),
                Err(InputPersistError::InvalidState)
            ),
            "case {case}"
        );
    }
}

#[cfg(target_arch = "x86_64")]
fn saved_input_manager(
    transport: Option<crate::pci::PciTransportMode>,
    present: bool,
) -> (
    crate::device_manager::DevicesState,
    crate::vstate::resources::ResourceAllocatorState,
    Option<super::persist::InputState>,
) {
    use crate::builder::tests::default_kernel_cmdline;
    use crate::device_manager::DeviceManager;
    use crate::utils::mib_to_bytes;
    use crate::vstate::vm::{Vm, tests::setup_vm_with_memory};
    let mut source = setup_vm_with_memory(mib_to_bytes(128));
    source.setup_irqchip(true).unwrap();
    let source = Arc::new(source);
    source.resource_allocator().pci_transport = transport.unwrap_or_default();
    let mut events = crate::EventManager::new().unwrap();
    let mut manager = DeviceManager::new(
        &mut events,
        source.vcpus_exit_evt(),
        &source,
        None,
        None,
        transport.is_some(),
        false,
    )
    .unwrap();
    manager.attach_vmgenid_device(&source).unwrap();
    manager.attach_vmclock_device(&source).unwrap();
    let expected = if present {
        let input = Arc::new(Mutex::new(Input::new().unwrap()));
        input.lock().unwrap().key(125, true).unwrap();
        manager
            .attach_boot_virtio_device(
                &Vm::Kvm(source.clone()),
                super::INPUT_DEV_ID.to_string(),
                input.clone(),
                &mut default_kernel_cmdline(),
                &mut events,
                false,
            )
            .unwrap();
        Some(input.lock().unwrap().save())
    } else {
        None
    };
    let state = manager.save().unwrap();
    let bytes = bitcode::serialize(&state).unwrap();
    let allocation = source.resource_allocator().save();
    (bitcode::deserialize(&bytes).unwrap(), allocation, expected)
}

#[cfg(target_arch = "x86_64")]
fn input_restore_vm(
    allocation: &crate::vstate::resources::ResourceAllocatorState,
) -> Arc<crate::vstate::vm::KvmVm> {
    use crate::utils::mib_to_bytes;
    use crate::vstate::resources::ResourceAllocator;
    use crate::vstate::vm::tests::setup_vm_with_memory;

    let mut target = setup_vm_with_memory(mib_to_bytes(128));
    target.setup_irqchip(true).unwrap();
    let target = Arc::new(target);
    *target.resource_allocator() = ResourceAllocator::restore((), allocation).unwrap();
    target
}

#[cfg(target_arch = "x86_64")]
#[test]
fn transport_managers_restore_input_state_and_default_to_no_device() {
    use crate::device_manager::{DeviceManager, DeviceRestoreArgs};
    use crate::devices::virtio::device::VirtioDeviceType;
    use crate::pci::PciTransportMode;
    use crate::resources::VmResources;

    for transport in [
        None,
        Some(PciTransportMode::Modern),
        Some(PciTransportMode::Transitional),
    ] {
        for present in [false, true] {
            let (saved, allocation, expected) = saved_input_manager(transport, present);
            let target = input_restore_vm(&allocation);
            let mut events = crate::EventManager::new().unwrap();
            let mut resources = VmResources::default();
            let manager = DeviceManager::restore(
                DeviceRestoreArgs {
                    vm: &target,
                    mem: target.guest_memory(),
                    vcpus_exit_evt: target.vcpus_exit_evt(),
                    event_manager: &mut events,
                    vm_resources: &mut resources,
                    instance_id: "input-restore",
                    vmgenid_notification: Default::default(),
                    vmclock_notification: Default::default(),
                },
                &saved,
            )
            .unwrap();
            let actual = manager
                .get_virtio_device(VirtioDeviceType::Input, super::INPUT_DEV_ID)
                .map(|device| {
                    device
                        .lock()
                        .unwrap()
                        .as_any()
                        .downcast_ref::<Input>()
                        .unwrap()
                        .save()
                });
            assert_eq!(actual, expected);
            if transport.is_none() {
                assert_eq!(
                    target
                        .resource_allocator()
                        .gsi_legacy_allocator
                        .is_allocated(7),
                    present
                );
            }
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[test]
fn activated_input_with_unready_queues_rejects_before_mmio_registration() {
    use super::persist::InputPersistError;
    use crate::device_manager::mmio::MMIOVirtioDevices;
    use crate::device_manager::persist::MMIODevManagerConstructorArgs;
    use crate::device_manager::{DevicePersistError, VirtioDevicesState};
    use crate::resources::VmResources;

    let (mut saved, allocation, _) = saved_input_manager(None, true);
    let VirtioDevicesState::Mmio(state) = &mut saved.virtio_state else {
        panic!("expected MMIO");
    };
    let input = &mut state
        .input_device
        .as_mut()
        .unwrap()
        .device_state
        .virtio_state;
    input.activated = true;
    input.acked_features = super::device::FEATURES;
    let target = input_restore_vm(&allocation);
    let mut events = crate::EventManager::new().unwrap();
    let mut resources = VmResources::default();
    let open_fds = || std::fs::read_dir("/proc/self/fd").unwrap().count();
    let before = open_fds();
    for status_ready in [false, true] {
        let mut invalid = state.clone();
        let mut status = crate::devices::virtio::queue::Queue::new(
            crate::devices::virtio::queue::FIRECRACKER_MAX_QUEUE_SIZE,
        );
        status.ready = status_ready;
        invalid
            .input_device
            .as_mut()
            .unwrap()
            .device_state
            .virtio_state
            .queues[STATUS_QUEUE] = status.save();
        for _ in 0..3 {
            let result = MMIOVirtioDevices::restore(
                MMIODevManagerConstructorArgs {
                    vm: &target,
                    mem: target.guest_memory(),
                    event_manager: &mut events,
                    vm_resources: &mut resources,
                    instance_id: "invalid-input",
                },
                &invalid,
            );
            assert_eq!(
                open_fds(),
                before,
                "failed restore retained input descriptors/subscriber"
            );
            assert!(matches!(
                result,
                Err(DevicePersistError::Input(InputPersistError::InvalidState))
            ));
            assert!(
                !target
                    .resource_allocator()
                    .gsi_legacy_allocator
                    .is_allocated(7)
            );
        }
    }
}

#[test]
fn maximum_bounded_input_state_round_trips() {
    use super::persist::{InputConstructorArgs, InputState};

    let state = InputState::maximum_test_state();
    assert_eq!(state.pending.len(), MAX_PENDING_FRAMES);
    assert!(
        state
            .pending
            .iter()
            .all(|frame| frame.events.len() == MAX_FRAME_EVENTS)
    );
    let decoded = bitcode::deserialize(&bitcode::serialize(&state).unwrap()).unwrap();
    let restored = Input::restore(InputConstructorArgs { mem: default_mem() }, &decoded).unwrap();
    assert_eq!(restored.save(), state);
}

#[test]
fn disconnect_cancels_an_undelivered_lock_make_without_waiting_for_its_led() {
    let mem = default_mem();
    let (mut device, queues) = active_input(&mem, 8);
    device.key(58, true).unwrap();
    assert_eq!(device.pending_led_state(), 2);
    device.release_all().unwrap();
    event_buffers(&queues[EVENT_QUEUE], 4, 0x8000);
    device.flush_events().unwrap();
    assert_eq!(read_event(&mem, 0x8020), InputEvent::new(EV_KEY, 58, 0));
    assert_eq!(
        device.pending_led_state(),
        0,
        "cancelled make cannot receive a guest LED acknowledgement"
    );
    event_buffers(&queues[EVENT_QUEUE], 2, 0x8000);
    device.key(58, true).unwrap();
    status_buffer(
        &mem,
        &queues[STATUS_QUEUE],
        0,
        InputEvent::new(EV_LED, 1, 1),
        8,
        0,
    );
    device.process_status().unwrap();
    assert_eq!(device.led_state(), 2);
    assert_eq!(device.pending_led_state(), 0);
}

#[test]
fn disconnect_preserves_written_lock_feedback_when_a_later_toggle_is_cancelled() {
    let mem = default_mem();
    let (mut device, queues) = input_with_key_down(&mem, 58);
    device.key(58, false).unwrap();
    device.key(58, true).unwrap();
    device.release_all().unwrap();
    let mut device = restore_active_input(device, &mem);
    assert_eq!(device.pending_led_state(), 2);
    event_buffers(&queues[EVENT_QUEUE], 4, 0x9000);
    device.flush_events().unwrap();
    status_buffer(
        &mem,
        &queues[STATUS_QUEUE],
        0,
        InputEvent::new(EV_LED, 1, 1),
        8,
        0,
    );
    device.process_status().unwrap();
    assert_eq!(device.led_state(), 2);
    assert_eq!(
        device.pending_led_state(),
        0,
        "written make still expects its original guest acknowledgement"
    );
}

#[test]
fn disconnect_completes_a_started_report_then_releases_its_projected_buttons() {
    let mem = default_mem();
    let (mut device, queues) = active_input(&mem, 8);
    offer(&queues[EVENT_QUEUE], 0, 0x8000, 8, VIRTQ_DESC_F_WRITE);
    device.pointer(1, 2, 1, 0).unwrap();
    device.pointer(3, 4, 0, 0).unwrap();
    device.release_all().unwrap();
    event_buffers(&queues[EVENT_QUEUE], 7, 0x9000);
    device.flush_events().unwrap();
    let actual: Vec<_> = (0..7)
        .map(|index| read_event(&mem, 0x9000 + index * 16))
        .collect();
    assert_eq!(
        actual,
        [
            InputEvent::new(EV_ABS, 1, 2),
            InputEvent::new(EV_KEY, BTN_LEFT, 1),
            InputEvent::SYN,
            InputEvent::new(EV_ABS, 0, 3),
            InputEvent::new(EV_ABS, 1, 4),
            InputEvent::new(EV_KEY, BTN_LEFT, 0),
            InputEvent::SYN,
        ]
    );
    assert!(device.pending.is_empty());
    assert!(device.held.is_empty() && device.delivered.is_empty());
}

#[test]
fn event_queue_only_activates_and_ignores_status_notifications() {
    let mem = default_mem();
    let queues = virtqueues(&mem, 8);
    let mut device = Input::new().unwrap();
    device.set_acked_features(super::device::FEATURES);
    assert!(matches!(
        device.activate(mem.clone(), Arc::new(IrqTrigger::new())),
        Err(crate::devices::virtio::ActivateError::QueueMemoryError(
            crate::devices::virtio::queue::QueueError::NotReady
        ))
    ));
    configure_input(&mut device, &queues);
    device.queues[STATUS_QUEUE].desc_table_address = GuestAddress(!15_u64);
    assert!(matches!(
        device.activate(mem.clone(), Arc::new(IrqTrigger::new())),
        Err(crate::devices::virtio::ActivateError::QueueMemoryError(
            crate::devices::virtio::queue::QueueError::MemoryError(_)
        ))
    ));
    assert!(!device.is_activated());
    device.queues[STATUS_QUEUE].ready = false;
    device
        .activate(mem.clone(), Arc::new(IrqTrigger::new()))
        .unwrap();
    event_buffers(&queues[EVENT_QUEUE], 2, 0x8000);
    device.key(30, true).unwrap();
    assert_eq!(read_event(&mem, 0x8000), InputEvent::new(EV_KEY, 30, 1));
    assert_eq!(read_event(&mem, 0x8010), InputEvent::SYN);
    let (device, mut events) = registered_input(device);
    device.lock().unwrap().queue_events()[STATUS_QUEUE]
        .write(1)
        .unwrap();
    events.run_with_timeout(0).unwrap();
    assert_eq!(device.lock().unwrap().led_state(), 0);
    assert_eq!(queues[STATUS_QUEUE].used.idx.get(), 0);
}

#[test]
fn event_queue_only_state_restores_and_status_can_be_configured_after_reset() {
    let mem = default_mem();
    let queues = virtqueues(&mem, 8);
    let mut device = Input::new().unwrap();
    configure_input(&mut device, &queues);
    device.queues[STATUS_QUEUE].ready = false;
    device
        .activate(mem.clone(), Arc::new(IrqTrigger::new()))
        .unwrap();
    device.key(125, true).unwrap();
    let mut device = restore_active_input(device, &mem);
    assert!(device.queues[EVENT_QUEUE].is_initialized());
    assert!(!device.queues[STATUS_QUEUE].is_initialized());
    assert_eq!(device.pending_frames(), 1);
    assert!(device.reset());
    let queues = virtqueues(&mem, 8);
    activate_input(&mut device, &mem, &queues);
    assert!(device.queues[STATUS_QUEUE].is_initialized());
    status_buffer(
        &mem,
        &queues[STATUS_QUEUE],
        0,
        InputEvent::new(EV_LED, 0, 1),
        8,
        0,
    );
    device.process_status().unwrap();
    assert_eq!(device.led_state(), 1);
}

#[test]
fn ready_status_queue_processes_buffers_added_after_input_activation() {
    let mem = default_mem();
    let (device, queues) = active_input(&mem, 8);
    let (device, mut events) = registered_input(device);
    events.run_with_timeout(0).unwrap();
    assert_eq!(device.lock().unwrap().led_state(), 0);
    status_buffer(
        &mem,
        &queues[STATUS_QUEUE],
        0,
        InputEvent::new(EV_LED, 1, 1),
        8,
        0,
    );
    device.lock().unwrap().queue_events()[STATUS_QUEUE]
        .write(1)
        .unwrap();
    events.run_with_timeout(0).unwrap();
    assert_eq!(device.lock().unwrap().led_state(), 2);
    assert_eq!(queues[STATUS_QUEUE].used.idx.get(), 1);
}
