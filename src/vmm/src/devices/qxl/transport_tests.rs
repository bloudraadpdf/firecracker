// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use vm_memory::MmapRegion;

use super::memory::*;
use super::protocol::DecodeError;
use super::transport::*;

const RAM: u64 = 0x8000_0000;
const VRAM: u64 = 0x8200_0000;

fn setup() -> (Memory, AddressSpace, Queues) {
    let memory = Memory::new(Arc::new(MmapRegion::new(SURFACE_BYTES).unwrap())).unwrap();
    let mut addresses = AddressSpace::new(RAM, VRAM).unwrap();
    assert_eq!(addresses.add(1, RAM..RAM + 4096), Ok(1));
    (memory, addresses, Queues::default())
}

fn pointer(generation: u8, offset: u64) -> u64 {
    (1 << 56) | (u64::from(generation) << 48) | offset
}

fn submit(memory: &Memory, sequence: u32, kind: u32) {
    let address = pointer(1, u64::from(sequence) * 16);
    let id = u64::from(sequence) + 100;
    memory
        .write(
            Region::Ram,
            usize::try_from(sequence).unwrap() * 16,
            &id.to_le_bytes(),
        )
        .unwrap();
    let mut command = [0; 16];
    command[..8].copy_from_slice(&address.to_le_bytes());
    command[8..12].copy_from_slice(&kind.to_le_bytes());
    memory
        .write(
            Region::Ram,
            COMMAND_RING + 20 + usize::try_from(sequence & 31).unwrap() * 16,
            &command,
        )
        .unwrap();
    memory
        .store(COMMAND_RING + 4, sequence.wrapping_add(1))
        .unwrap();
}

#[test]
fn slot_bounds_exclude_control_memory_and_reject_stale_generations() {
    let (_, mut addresses, _) = setup();
    assert_eq!(
        addresses.resolve(pointer(1, 4095), 1),
        Ok((Region::Ram, 4095))
    );
    assert_eq!(
        addresses.resolve(pointer(1, 4095), 2),
        Err(DecodeError::Address)
    );
    addresses.add(1, RAM..RAM + 4096).unwrap_err();
    addresses.remove(1).unwrap();
    addresses.resolve(pointer(1, 0), 1).unwrap_err();
    addresses
        .add(1, RAM..RAM + u64::try_from(RAM_BYTES).unwrap())
        .unwrap_err();
    assert_eq!(addresses.add(1, RAM + 4096..RAM + 8192), Ok(2));
    addresses.resolve(pointer(1, 0), 1).unwrap_err();
    assert_eq!(addresses.resolve(pointer(2, 0), 1), Ok((Region::Ram, 4096)));
    addresses.reset();
    assert_eq!(addresses.add(1, VRAM..VRAM + 4096), Ok(3));
    assert_eq!(addresses.resolve(pointer(3, 0), 1), Ok((Region::Vram, 0)));
    AddressSpace::new(RAM, RAM).unwrap_err();
    AddressSpace::new(RAM + 1, VRAM).unwrap_err();
    AddressSpace::new(RAM, u64::MAX - ((1 << 25) - 1)).unwrap_err();
}

#[test]
fn completed_commands_publish_release_ids_and_terminate_the_guest_chain() {
    let (memory, addresses, mut queues) = setup();
    submit(&memory, 0, 1);
    memory.store(COMMAND_RING + 16, 1).unwrap();
    let mut executed = Vec::new();
    assert_eq!(
        queues
            .drain(&memory, &addresses, Queue::Commands, |command| {
                executed.push(command.address);
                Ok(())
            })
            .unwrap(),
        1
    );
    assert_eq!(executed, [pointer(1, 0)]);
    assert_eq!(memory.load(COMMAND_RING + 12).unwrap(), 1);
    assert_eq!(memory.load(COMMAND_RING + 8).unwrap(), 2);
    assert_eq!(memory.load(RELEASE_RING + 4).unwrap(), 1);
    assert_eq!(memory.load(RAM_HEADER + 4).unwrap(), 1);
    let mut id = [0; 8];
    memory
        .read(Region::Ram, RELEASE_RING + 20, &mut id)
        .unwrap();
    assert_eq!(u64::from_le_bytes(id), 100);
    addresses.read(&memory, pointer(1, 0), &mut id).unwrap();
    assert_eq!(id, [0; 8]);
}

#[test]
fn a_full_command_dispatch_publishes_one_complete_release_chain() {
    let (memory, addresses, mut queues) = setup();
    for sequence in 0..32 {
        submit(&memory, sequence, 1);
    }
    assert_eq!(
        queues
            .drain(&memory, &addresses, Queue::Commands, |_| Ok(()))
            .unwrap(),
        32
    );
    assert_eq!(memory.load(RELEASE_RING + 4).unwrap(), 1);
    for sequence in 0..32 {
        let mut next = [0; 8];
        addresses
            .read(&memory, pointer(1, sequence * 16), &mut next)
            .unwrap();
        assert_eq!(
            u64::from_le_bytes(next),
            if sequence == 31 { 0 } else { sequence + 101 }
        );
    }
}

#[test]
fn full_release_ring_stops_before_execution_and_resumes_without_duplicates() {
    let (memory, addresses, mut queues) = setup();
    let mut executed = Vec::new();
    let mut execute = |command: super::protocol::Command| {
        executed.push(command.address);
        Ok(())
    };
    for sequence in 0..8 {
        submit(&memory, sequence, 1);
        assert_eq!(
            queues
                .drain(&memory, &addresses, Queue::Commands, &mut execute)
                .unwrap(),
            1
        );
    }
    for sequence in 8..12 {
        submit(&memory, sequence, 1);
    }
    assert_eq!(
        queues
            .drain(&memory, &addresses, Queue::Commands, &mut execute)
            .unwrap(),
        0
    );
    assert_eq!(memory.load(COMMAND_RING + 12).unwrap(), 8);
    assert_eq!(memory.load(RELEASE_RING + 16).unwrap(), 1);
    memory.store(RELEASE_RING + 12, 4).unwrap();
    assert_eq!(
        queues
            .drain(&memory, &addresses, Queue::Commands, &mut execute)
            .unwrap(),
        4
    );
    assert_eq!(executed.len(), 12);
    for (sequence, address) in executed.into_iter().enumerate() {
        assert_eq!(address, pointer(1, u64::try_from(sequence).unwrap() * 16));
    }
}

#[test]
fn a_failed_later_command_publishes_only_the_successful_prefix() {
    let (memory, addresses, mut queues) = setup();
    for sequence in 0..3 {
        submit(&memory, sequence, 1);
    }
    queues
        .drain(&memory, &addresses, Queue::Commands, |command| {
            if command.address == pointer(1, 16) {
                Err(DecodeError::Parameter.into())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
    assert_eq!(memory.load(COMMAND_RING + 12).unwrap(), 1);
    assert_eq!(memory.load(RELEASE_RING + 4).unwrap(), 1);
    let mut id = [0; 8];
    addresses.read(&memory, pointer(1, 0), &mut id).unwrap();
    assert_eq!(id, [0; 8]);
    addresses.read(&memory, pointer(1, 16), &mut id).unwrap();
    assert_eq!(u64::from_le_bytes(id), 101);
    assert_eq!(
        queues
            .drain(&memory, &addresses, Queue::Commands, |_| Ok(()))
            .unwrap(),
        2
    );
    assert_eq!(memory.load(RELEASE_RING + 4).unwrap(), 2);
    addresses.read(&memory, pointer(1, 16), &mut id).unwrap();
    assert_eq!(u64::from_le_bytes(id), 102);
}

#[test]
fn duplicate_release_addresses_do_not_form_cycles_or_execute_twice() {
    let (memory, addresses, mut queues) = setup();
    submit(&memory, 0, 1);
    submit(&memory, 1, 1);
    memory
        .write(Region::Ram, COMMAND_RING + 36, &pointer(1, 0).to_le_bytes())
        .unwrap();
    let mut executed = 0;
    queues
        .drain(&memory, &addresses, Queue::Commands, |_| {
            executed += 1;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(executed, 1);
    assert_eq!(memory.load(COMMAND_RING + 12).unwrap(), 1);
    let mut next = [0; 8];
    addresses.read(&memory, pointer(1, 0), &mut next).unwrap();
    assert_eq!(next, [0; 8]);
}

#[test]
fn renderer_failure_does_not_acknowledge_or_release_the_command() {
    let (memory, addresses, mut queues) = setup();
    submit(&memory, 0, 1);
    let result = queues.drain(&memory, &addresses, Queue::Commands, |_| {
        Err(DecodeError::Parameter.into())
    });
    assert!(matches!(
        result,
        Err(TransportError::Decode(DecodeError::Parameter))
    ));
    assert_eq!(memory.load(COMMAND_RING + 12).unwrap(), 0);
    assert_eq!(memory.load(RELEASE_RING + 4).unwrap(), 0);
    let mut id = [0; 8];
    addresses.read(&memory, pointer(1, 0), &mut id).unwrap();
    assert_eq!(u64::from_le_bytes(id), 100);
}

#[test]
fn forged_release_indices_and_invalid_pointers_never_reach_the_renderer() {
    for (offset, value) in [
        (RELEASE_RING, 16),
        (RELEASE_RING + 4, 1),
        (RELEASE_RING + 12, 1),
    ] {
        let (memory, addresses, mut queues) = setup();
        submit(&memory, 0, 1);
        memory.store(offset, value).unwrap();
        queues
            .drain(&memory, &addresses, Queue::Commands, |_| {
                panic!("invalid ring executed")
            })
            .unwrap_err();
    }
    let (memory, addresses, mut queues) = setup();
    submit(&memory, 0, 1);
    memory
        .write(Region::Ram, COMMAND_RING + 20, &pointer(2, 0).to_le_bytes())
        .unwrap();
    queues
        .drain(&memory, &addresses, Queue::Commands, |_| {
            panic!("stale pointer executed")
        })
        .unwrap_err();
}

#[test]
fn one_dispatch_consumes_at_most_the_fixed_ring_capacity() {
    let (memory, addresses, mut queues) = setup();
    for sequence in 0..32 {
        submit(&memory, sequence, 0);
    }
    assert_eq!(
        queues
            .drain(&memory, &addresses, Queue::Commands, |_| panic!(
                "NOP needs no renderer"
            ))
            .unwrap(),
        32
    );
    assert_eq!(memory.load(COMMAND_RING + 12).unwrap(), 32);
    assert_eq!(memory.load(COMMAND_RING + 8).unwrap(), 33);
    assert_eq!(memory.load(RELEASE_RING + 4).unwrap(), 0);
}

#[test]
fn cursor_commands_use_the_cursor_queue_and_its_notification_bit() {
    let (memory, addresses, mut queues) = setup();
    submit(&memory, 0, 3);
    queues
        .drain(&memory, &addresses, Queue::Commands, |_| {
            panic!("wrong queue executed")
        })
        .unwrap_err();
    let mut command = [0; 16];
    memory
        .read(Region::Ram, COMMAND_RING + 20, &mut command)
        .unwrap();
    memory
        .write(Region::Ram, CURSOR_RING + 20, &command)
        .unwrap();
    memory.store(CURSOR_RING + 4, 1).unwrap();
    memory.store(CURSOR_RING + 16, 1).unwrap();
    memory.store(RELEASE_RING + 8, 2).unwrap();
    let mut count = 0;
    assert_eq!(
        queues
            .drain(&memory, &addresses, Queue::Cursor, |_| {
                count += 1;
                Ok(())
            })
            .unwrap(),
        1
    );
    assert_eq!(count, 1);
    assert_eq!(memory.load(RAM_HEADER + 4).unwrap(), 2);
}
