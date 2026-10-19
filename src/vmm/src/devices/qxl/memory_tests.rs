// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use vm_memory::{Bytes, MmapRegion, VolatileMemory};

use super::memory::*;
use super::protocol::{Ring, RingSize};

fn memory() -> Memory {
    Memory::new(Arc::new(MmapRegion::new(SURFACE_BYTES).unwrap())).unwrap()
}

fn word(memory: &Memory, region: Region, offset: usize) -> u32 {
    let mut bytes = [0; 4];
    memory.read(region, offset, &mut bytes).unwrap();
    u32::from_le_bytes(bytes)
}

#[test]
fn rom_describes_the_actual_banks_and_complete_bounded_mode_table() {
    let memory = memory();
    assert_eq!(word(&memory, Region::Rom, 0), u32::from_le_bytes(*b"QXRO"));
    assert_eq!(word(&memory, Region::Rom, 40), 16 << 20);
    assert_eq!(word(&memory, Region::Rom, 44), (32 << 20) - 8192);
    assert_eq!(word(&memory, Region::Rom, 28), 4094);
    assert_eq!(word(&memory, Region::Rom, 52), 1);
    let mut slots = [0; 6];
    memory.read(Region::Rom, 64, &mut slots).unwrap();
    assert_eq!(slots, [1, 3, 8, 8, 0, 1]);
    let start = usize::try_from(word(&memory, Region::Rom, 24)).unwrap();
    let count = usize::try_from(word(&memory, Region::Rom, start)).unwrap();
    assert_eq!(count, MODES.len());
    assert!(start + 4 + count * 32 <= ROM_BYTES);
    for (index, &(width, height)) in MODES.iter().enumerate() {
        let mode = start + 4 + index * 32;
        assert_eq!(
            word(&memory, Region::Rom, mode),
            u32::try_from(index).unwrap()
        );
        assert_eq!(word(&memory, Region::Rom, mode + 4), width);
        assert_eq!(word(&memory, Region::Rom, mode + 8), height);
        assert_eq!(word(&memory, Region::Rom, mode + 12), 32);
        assert_eq!(word(&memory, Region::Rom, mode + 16), width * 4);
        assert!(u64::from(width) * u64::from(height) * 4 <= u64::try_from(SURFACE_BYTES).unwrap());
    }
}

#[test]
fn ram_header_offsets_and_initial_notifications_match_the_packed_abi() {
    let memory = memory();
    assert_eq!(COMMAND_RING - RAM_HEADER, 4108);
    assert_eq!(CURSOR_RING - RAM_HEADER, 4640);
    assert_eq!(RELEASE_RING - RAM_HEADER, 5172);
    assert_eq!(UPDATE_AREA - RAM_HEADER, 5256);
    assert_eq!(UPDATE_SURFACE - RAM_HEADER, 5272);
    assert_eq!(MEMORY_SLOT - RAM_HEADER, 5276);
    assert_eq!(CREATE_SURFACE - RAM_HEADER, 5292);
    assert_eq!(FLAGS - RAM_HEADER, 5332);
    assert_eq!(MONITORS_CONFIG - RAM_HEADER, 5340);
    assert_eq!(
        word(&memory, Region::Ram, RAM_HEADER),
        u32::from_le_bytes(*b"QXRA")
    );
    for (offset, size) in [
        (COMMAND_RING, RingSize::Commands),
        (CURSOR_RING, RingSize::Commands),
        (RELEASE_RING, RingSize::Releases),
    ] {
        let mut bytes = [0; 20];
        memory.read(Region::Ram, offset, &mut bytes).unwrap();
        assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), 1);
        assert_eq!(Ring::decode(&bytes, size, 0).unwrap().pending(), 0);
    }
}

#[test]
fn split_ram_reads_and_writes_preserve_the_shared_boot_mapping() {
    let boot = Arc::new(MmapRegion::new(SURFACE_BYTES).unwrap());
    let memory = Memory::new(boot.clone()).unwrap();
    memory
        .write(Region::Ram, SURFACE_BYTES - 2, &[1, 2, 3, 4])
        .unwrap();
    let mut bytes = [0; 4];
    memory
        .read(Region::Ram, SURFACE_BYTES - 2, &mut bytes)
        .unwrap();
    assert_eq!(bytes, [1, 2, 3, 4]);
    let mut shared = [0; 2];
    boot.get_slice(SURFACE_BYTES - 2, 2)
        .unwrap()
        .read_slice(&mut shared, 0)
        .unwrap();
    assert_eq!(shared, [1, 2]);
    boot.get_slice(0, 2)
        .unwrap()
        .write_slice(&[5, 6], 0)
        .unwrap();
    memory.read(Region::Ram, 0, &mut shared).unwrap();
    assert_eq!(shared, [5, 6]);
    memory.write(Region::Vram, 0, &[7, 8]).unwrap();
    memory.read(Region::Ram, 0, &mut shared).unwrap();
    assert_eq!(shared, [5, 6]);
}

#[test]
fn invalid_memory_accesses_are_rejected_before_changing_any_bank() {
    let memory = memory();
    memory.write(Region::Ram, RAM_BYTES - 1, &[42]).unwrap();
    memory
        .write(Region::Ram, RAM_BYTES - 1, &[1, 2])
        .unwrap_err();
    memory.write(Region::Ram, usize::MAX, &[1]).unwrap_err();
    memory.write(Region::Rom, 0, &[0; 4]).unwrap_err();
    memory.load(1).unwrap_err();
    memory.store(RAM_BYTES - 2, 1).unwrap_err();
    let mut byte = [0];
    memory.read(Region::Ram, RAM_BYTES - 1, &mut byte).unwrap();
    assert_eq!(byte, [42]);
    memory
        .read(Region::Vram, VRAM_BYTES, &mut byte)
        .unwrap_err();
    memory.read(Region::Rom, ROM_BYTES, &mut byte).unwrap_err();
    for region in [Region::Ram, Region::Vram, Region::Rom] {
        memory.read(region, Memory::size(region), &mut []).unwrap();
    }
    Memory::new(Arc::new(MmapRegion::new(4096).unwrap())).unwrap_err();
}

#[test]
fn reset_reinitialises_control_state_without_erasing_guest_pixels() {
    let memory = memory();
    memory.write(Region::Ram, 0, &[1, 2, 3, 4]).unwrap();
    memory.write(Region::Vram, 0, &[5, 6, 7, 8]).unwrap();
    memory.interrupt(1).unwrap();
    memory.interrupt(4).unwrap();
    assert_eq!(memory.load(RAM_HEADER + 4).unwrap(), 5);
    memory.store(COMMAND_RING + 4, 7).unwrap();
    memory.reset(19).unwrap();
    assert_eq!(memory.load(RAM_HEADER + 4).unwrap(), 0);
    assert_eq!(memory.load(COMMAND_RING + 4).unwrap(), 0);
    let mut generation = [0];
    memory.read(Region::Rom, 68, &mut generation).unwrap();
    assert_eq!(generation, [19]);
    let mut bytes = [0; 4];
    memory.read(Region::Ram, 0, &mut bytes).unwrap();
    assert_eq!(bytes, [1, 2, 3, 4]);
    memory.read(Region::Vram, 0, &mut bytes).unwrap();
    assert_eq!(bytes, [5, 6, 7, 8]);
    let regions = memory.regions();
    assert_eq!(
        regions
            .iter()
            .map(|(_, _, mapping)| mapping.size())
            .sum::<usize>(),
        RAM_BYTES + VRAM_BYTES + ROM_BYTES
    );
    assert_eq!((regions[0].0, regions[0].1), (Region::Ram, 0));
    assert_eq!((regions[1].0, regions[1].1), (Region::Ram, SURFACE_BYTES));
}
