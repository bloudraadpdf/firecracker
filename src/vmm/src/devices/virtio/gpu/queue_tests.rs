// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use super::super::test_utils::request;
use super::*;
use crate::devices::virtio::queue::VIRTQ_DESC_F_NEXT;
use crate::devices::virtio::test_utils::{VirtQueue as TestQueue, default_mem};

fn descriptors<'a>(
    mem: &'a GuestMemoryMmap,
    bytes: &[u8],
    capacity: u32,
) -> (TestQueue<'a>, VirtQueue) {
    let ring = TestQueue::new(GuestAddress(0x1000), mem, 8);
    mem.write_slice(bytes, GuestAddress(0x8000)).unwrap();
    ring.dtable[0].set(
        0x8000,
        u32::try_from(bytes.len()).unwrap(),
        VIRTQ_DESC_F_NEXT,
        1,
    );
    ring.dtable[1].set(0x9000, capacity, VIRTQ_DESC_F_WRITE, 0);
    ring.avail.ring[0].set(0);
    ring.avail.idx.set(1);
    let queue = ring.create_queue();
    (ring, queue)
}

fn response(mem: &GuestMemoryMmap, size: usize) -> Vec<u8> {
    let mut bytes = vec![0; size];
    mem.read_slice(&mut bytes, GuestAddress(0x9000)).unwrap();
    bytes
}

#[test]
fn display_info_returns_one_connected_output_and_only_its_response_bytes() {
    let mem = default_mem();
    mem.write_slice(&[0xff; 512], GuestAddress(0x9000)).unwrap();
    let bytes = request(0x0100, &[]);
    let (ring, mut queue) = descriptors(&mem, &bytes, 512);
    let mut renderer = Renderer::new(1024, 768).unwrap();
    assert!(process_queue(&mut queue, Queue::Control, &mut renderer, &mem).unwrap());
    ring.check_used_elem(0, 0, u32::try_from(DISPLAY_INFO_SIZE).unwrap());
    let bytes = response(&mem, 512);
    assert_eq!(&bytes[..4], &0x1101u32.to_le_bytes());
    assert_eq!(&bytes[32..36], &1024u32.to_le_bytes());
    assert_eq!(&bytes[36..40], &768u32.to_le_bytes());
    assert_eq!(&bytes[40..44], &1u32.to_le_bytes());
    assert_eq!(&bytes[48..DISPLAY_INFO_SIZE], &[0; DISPLAY_INFO_SIZE - 48]);
    assert_eq!(
        &bytes[DISPLAY_INFO_SIZE..],
        &[0xff; 512 - DISPLAY_INFO_SIZE]
    );
    assert!(!process_queue(&mut queue, Queue::Control, &mut renderer, &mem).unwrap());
}

#[test]
fn split_requests_and_responses_preserve_fence_ids() {
    let mem = default_mem();
    let mut bytes = request(0x0101, &[1, 2, 1024, 768]);
    bytes[4..8].copy_from_slice(&1u32.to_le_bytes());
    bytes[8..16].copy_from_slice(&0xfedcba9887654321u64.to_le_bytes());
    let (ring, mut queue) = descriptors(&mem, &bytes, 24);
    mem.write_slice(&bytes[9..], GuestAddress(0x8100)).unwrap();
    ring.dtable[0].set(0x8000, 9, VIRTQ_DESC_F_NEXT, 1);
    ring.dtable[1].set(0x8100, 31, VIRTQ_DESC_F_NEXT, 2);
    ring.dtable[2].set(0x9000, 7, VIRTQ_DESC_F_WRITE | VIRTQ_DESC_F_NEXT, 3);
    ring.dtable[3].set(0x9100, 17, VIRTQ_DESC_F_WRITE, 0);
    let mut renderer = Renderer::new(1024, 768).unwrap();
    process_queue(&mut queue, Queue::Control, &mut renderer, &mem).unwrap();
    ring.check_used_elem(0, 0, 24);
    let mut actual = response(&mem, 7);
    let mut tail = [0; 17];
    mem.read_slice(&mut tail, GuestAddress(0x9100)).unwrap();
    actual.extend(tail);
    assert_eq!(actual, Header::decode(&bytes).unwrap().response(0x1100));
}

#[test]
fn rejected_commands_still_complete_the_requested_fence() {
    let mem = default_mem();
    for (kind, flags, expected) in [(0x0100u32, 3u32, 0x1205u32), (0x0200, 1, 0x1200)] {
        let mut bytes = request(kind, &[]);
        bytes[4..8].copy_from_slice(&flags.to_le_bytes());
        bytes[8..16].copy_from_slice(&u64::MAX.to_le_bytes());
        let (ring, mut queue) = descriptors(&mem, &bytes, 24);
        let mut renderer = Renderer::new(1024, 768).unwrap();
        process_queue(&mut queue, Queue::Control, &mut renderer, &mem).unwrap();
        ring.check_used_elem(0, 0, 24);
        let actual = response(&mem, 24);
        assert_eq!(&actual[..4], &expected.to_le_bytes());
        assert_eq!(&actual[4..8], &1u32.to_le_bytes());
        assert_eq!(&actual[8..16], &u64::MAX.to_le_bytes());
    }
}

#[test]
fn malformed_descriptors_do_not_execute_the_command() {
    let mem = default_mem();
    let bytes = request(0x0101, &[1, 2, 1024, 768]);
    for case in 0..5 {
        let (ring, mut queue) = descriptors(&mem, &bytes, 24);
        match case {
            0 => ring.dtable[1].len.set(23),
            1 => ring.dtable[1].addr.set(u64::MAX),
            2 => {
                ring.dtable[1]
                    .flags
                    .set(VIRTQ_DESC_F_WRITE | VIRTQ_DESC_F_NEXT);
                ring.dtable[1].next.set(2);
                ring.dtable[2].set(0x8100, 4, 0, 0);
            }
            3 => ring.dtable[0].next.set(0),
            4 => ring.dtable[0].len.set(23),
            _ => unreachable!(),
        }
        let mut renderer = Renderer::new(1024, 768).unwrap();
        process_queue(&mut queue, Queue::Control, &mut renderer, &mem).unwrap();
        ring.check_used_elem(0, 0, 0);
        let (_, command) = Command::decode(Queue::Control, &bytes).unwrap();
        renderer.execute(command, &mem).unwrap();
    }
}

#[test]
fn oversized_commands_are_rejected_before_allocation_or_execution() {
    let mem = crate::test_utils::single_region_mem(0x40000);
    let bytes = request(0x0101, &[1, 2, 1024, 768]);
    let (ring, mut queue) = descriptors(&mem, &bytes, 24);
    ring.dtable[0]
        .len
        .set(u32::try_from(MAX_REQUEST + 1).unwrap());
    let mut renderer = Renderer::new(1024, 768).unwrap();
    process_queue(&mut queue, Queue::Control, &mut renderer, &mem).unwrap();
    ring.check_used_elem(0, 0, 0);
    let (_, command) = Command::decode(Queue::Control, &bytes).unwrap();
    renderer.execute(command, &mem).unwrap();
}

#[test]
fn cursor_queue_accepts_commands_without_a_response_buffer() {
    let mem = default_mem();
    let bytes = request(0x0301, &[0; 8]);
    let (ring, mut queue) = descriptors(&mem, &bytes, 0);
    let mut renderer = Renderer::new(1024, 768).unwrap();
    process_queue(&mut queue, Queue::Cursor, &mut renderer, &mem).unwrap();
    assert_eq!(ring.used.idx.get(), 1);
    ring.check_used_elem(0, 0, 0);
}
