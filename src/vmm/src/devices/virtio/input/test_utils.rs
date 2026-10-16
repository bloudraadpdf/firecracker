// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use super::Input;
use crate::devices::virtio::device::VirtioDevice;
use crate::devices::virtio::queue::{FIRECRACKER_MAX_QUEUE_SIZE, VIRTQ_DESC_F_WRITE};
use crate::devices::virtio::test_utils::VirtQueue;
use crate::devices::virtio::transport::mmio::IrqTrigger;
use crate::vstate::memory::{GuestAddress, GuestMemoryMmap};

pub(crate) fn active_input(mem: &GuestMemoryMmap, size: u16) -> (Input, [VirtQueue<'_>; 2]) {
    let queues = virtqueues(mem, size);
    let mut device = Input::new().unwrap();
    activate_input(&mut device, mem, &queues);
    (device, queues)
}

pub(crate) fn virtqueues(mem: &GuestMemoryMmap, size: u16) -> [VirtQueue<'_>; 2] {
    [
        VirtQueue::new(GuestAddress(0x1000), mem, size),
        VirtQueue::new(GuestAddress(0x4000), mem, size),
    ]
}

pub(crate) fn activate_input(
    device: &mut Input,
    mem: &GuestMemoryMmap,
    queues: &[VirtQueue<'_>; 2],
) {
    configure_input(device, queues);
    device
        .activate(mem.clone(), Arc::new(IrqTrigger::new()))
        .unwrap();
}

pub(super) fn configure_input(device: &mut Input, queues: &[VirtQueue<'_>; 2]) {
    device.queues = queues
        .iter()
        .map(|queue| {
            let mut queue = queue.create_queue();
            queue.max_size = FIRECRACKER_MAX_QUEUE_SIZE;
            queue
        })
        .collect();
    device.set_acked_features(super::device::FEATURES);
}

pub(crate) fn event_buffers(queue: &VirtQueue<'_>, count: u16, base: u64) {
    for index in 0..count {
        queue.dtable[usize::from(index)].set(
            base + u64::from(index) * 16,
            8,
            VIRTQ_DESC_F_WRITE,
            0,
        );
        let next = queue.avail.idx.get();
        queue.avail.ring[usize::from(next % queue.size())].set(index);
        queue.avail.idx.set(next.wrapping_add(1));
    }
}
