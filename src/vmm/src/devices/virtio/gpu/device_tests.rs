// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use event_manager::SubscriberOps;
use vm_memory::Bytes;

use super::super::persist::{GpuConstructorArgs, GpuState};
use super::super::protocol::Command;
use super::super::test_utils::request;
use super::*;
use crate::devices::virtio::queue::{VIRTQ_DESC_F_NEXT, VIRTQ_DESC_F_WRITE};
use crate::devices::virtio::test_utils::{VirtQueue, default_mem};
use crate::devices::virtio::transport::mmio::IrqTrigger;
use crate::snapshot::Persist;
use crate::vstate::memory::GuestAddress;

fn control_queue<'a>(gpu: &mut Gpu, mem: &'a GuestMemoryMmap) -> VirtQueue<'a> {
    let ring = VirtQueue::new(GuestAddress(0x1000), mem, 8);
    let mut queue = ring.create_queue();
    queue.max_size = FIRECRACKER_MAX_QUEUE_SIZE;
    gpu.queues[0] = queue;
    ring
}

fn enqueue(ring: &VirtQueue<'_>, index: u16, sequence: u16, bytes: &[u8]) {
    let source = 0x8000 + u64::from(index) * 256;
    let target = 0xa000 + u64::from(index) * 512;
    ring.memory()
        .write_slice(bytes, GuestAddress(source))
        .unwrap();
    ring.dtable[usize::from(index)].set(
        source,
        u32::try_from(bytes.len()).unwrap(),
        VIRTQ_DESC_F_NEXT,
        index + 1,
    );
    ring.dtable[usize::from(index + 1)].set(target, 408, VIRTQ_DESC_F_WRITE, 0);
    ring.avail.ring[usize::from(sequence)].set(index);
    ring.avail.idx.set(sequence + 1);
}

#[test]
fn activation_requires_modern_features_and_a_ready_control_queue() {
    let mem = default_mem();
    let interrupt = Arc::new(IrqTrigger::new());
    let mut gpu = Gpu::new(1024, 768).unwrap();
    assert_eq!(gpu.device_type(), VirtioDeviceType::Gpu);
    assert_eq!(gpu.id(), GPU_DEV_ID);
    assert!(matches!(
        gpu.activate(mem.clone(), interrupt.clone()),
        Err(ActivateError::RequiredFeatureNotAcked(_))
    ));
    gpu.set_acked_features(u64::MAX);
    assert_eq!(gpu.acked_features(), FEATURES);
    assert!(matches!(
        gpu.activate(mem.clone(), interrupt.clone()),
        Err(ActivateError::QueueMemoryError(_))
    ));
    control_queue(&mut gpu, &mem);
    gpu.activate(mem, interrupt).unwrap();
    assert!(gpu.is_activated());
    assert!(!gpu.queues[1].ready);
}

#[test]
fn registered_events_process_pending_work_after_activation() {
    let mem = default_mem();
    let mut gpu = Gpu::new(1024, 768).unwrap();
    gpu.check_registered().unwrap_err();
    let ring = control_queue(&mut gpu, &mem);
    enqueue(&ring, 0, 0, &request(0x0100, &[]));
    let gpu = Arc::new(Mutex::new(gpu));
    let mut events = crate::EventManager::new().unwrap();
    events.add_subscriber(gpu.clone());
    gpu.lock().unwrap().check_registered().unwrap();
    gpu.lock().unwrap().queue_events[0].write(1).unwrap();
    events.run_with_timeout(100).unwrap();
    assert_eq!(ring.used.idx.get(), 0);
    let interrupt = Arc::new(IrqTrigger::new());
    {
        let mut device = gpu.lock().unwrap();
        device.set_acked_features(FEATURES);
        device.activate(mem.clone(), interrupt.clone()).unwrap();
    }
    events.run_with_timeout(100).unwrap();
    ring.check_used_elem(0, 0, 408);
    assert!(interrupt.has_pending_interrupt(VirtioInterruptType::Queue(0)));
    let mut width = [0; 4];
    mem.read_slice(&mut width, GuestAddress(0xa000 + 32))
        .unwrap();
    assert_eq!(u32::from_le_bytes(width), 1024);
}

#[test]
fn display_events_clear_only_through_the_events_clear_register() {
    let mem = default_mem();
    let mut gpu = Gpu::new(1024, 768).unwrap();
    control_queue(&mut gpu, &mem);
    gpu.set_acked_features(FEATURES);
    gpu.resize(1280, 720).unwrap();
    let interrupt = Arc::new(IrqTrigger::new());
    gpu.activate(mem, interrupt.clone()).unwrap();
    assert!(interrupt.has_pending_interrupt(VirtioInterruptType::Config));
    interrupt.ack_interrupt(VirtioInterruptType::Config);
    gpu.resize(1280, 720).unwrap();
    assert!(!interrupt.has_pending_interrupt(VirtioInterruptType::Config));
    gpu.write_config(0, &[0; 16]);
    assert_eq!(gpu.config[0], 1);
    assert_eq!(gpu.config[8], 1);
    gpu.write_config(u64::MAX, &[255; 4]);
    gpu.write_config(4, &[1]);
    assert_eq!(gpu.config[0], 0);
    gpu.resize(1920, 1080).unwrap();
    assert!(interrupt.has_pending_interrupt(VirtioInterruptType::Config));
    assert_eq!(gpu.config[0], 1);
    gpu.resize(0, 0).unwrap_err();
}

#[test]
fn checkpoint_restores_queue_progress_resources_events_and_reset() {
    let mem = default_mem();
    let mut gpu = Gpu::new(1024, 768).unwrap();
    let ring = control_queue(&mut gpu, &mem);
    let create = request(0x0101, &[1, 2, 1, 1]);
    enqueue(&ring, 0, 0, &create);
    gpu.set_acked_features(FEATURES);
    gpu.activate(mem.clone(), Arc::new(IrqTrigger::new()))
        .unwrap();
    gpu.process_queue(0).unwrap();
    ring.check_used_elem(0, 0, 24);
    gpu.resize(1280, 720).unwrap();
    let mut bytes = Vec::new();
    crate::snapshot::Snapshot::new(gpu.save())
        .save(&mut bytes)
        .unwrap();
    let snapshot = crate::snapshot::Snapshot::<GpuState>::load(&mut bytes.as_slice()).unwrap();
    assert_eq!(snapshot.version(), &crate::persist::SNAPSHOT_VERSION);
    let state = snapshot.data;
    let mut restored = Gpu::restore(GpuConstructorArgs { mem: mem.clone() }, &state).unwrap();
    assert!(!restored.is_activated());
    let (_, command) = Command::decode(QueueKind::Control, &create).unwrap();
    assert!(matches!(
        restored.renderer.lock().unwrap().execute(command, &mem),
        Err(RendererError::Resource)
    ));
    let interrupt = Arc::new(IrqTrigger::new());
    restored.activate(mem.clone(), interrupt.clone()).unwrap();
    assert!(interrupt.has_pending_interrupt(VirtioInterruptType::Config));
    enqueue(&ring, 2, 1, &request(0x0100, &[]));
    restored.process_queue(0).unwrap();
    ring.check_used_elem(1, 2, 408);
    let mut width = [0; 4];
    mem.read_slice(&mut width, GuestAddress(0xa000 + 1024 + 32))
        .unwrap();
    assert_eq!(u32::from_le_bytes(width), 1280);
    assert!(restored.reset());
    assert!(!restored.is_activated());
    assert_eq!(restored.acked_features(), 0);
    assert!(restored.queues.iter().all(|queue| !queue.ready));
    assert!(!restored.renderer.lock().unwrap().is_native());
    assert_eq!(restored.config[0], 0);
}

#[test]
fn restore_rejects_invalid_features_events_type_and_queue_count() {
    let mem = default_mem();
    let mut gpu = Gpu::new(1024, 768).unwrap();
    control_queue(&mut gpu, &mem);
    gpu.set_acked_features(FEATURES);
    gpu.activate(mem.clone(), Arc::new(IrqTrigger::new()))
        .unwrap();
    let state = gpu.save();
    let cases: &[fn(&mut GpuState)] = &[
        |state| state.virtio_state.avail_features = 0,
        |state| state.virtio_state.acked_features = 0,
        |state| state.virtio_state.acked_features |= 1,
        |state| state.events = 2,
        |state| state.virtio_state.device_type = VirtioDeviceType::Input,
        |state| {
            state.virtio_state.queues.pop();
        },
    ];
    for modify in cases {
        let mut invalid = state.clone();
        modify(&mut invalid);
        Gpu::restore(GpuConstructorArgs { mem: mem.clone() }, &invalid).unwrap_err();
    }
}
