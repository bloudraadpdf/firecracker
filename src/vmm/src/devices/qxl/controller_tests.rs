// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use super::controller::Controller;
use super::drawing::Layout;
use super::memory::{
    COMMAND_RING, CREATE_SURFACE, CURSOR_RING, MEMORY_SLOT, RAM_HEADER, RELEASE_RING, Region,
    UPDATE_AREA, UPDATE_SURFACE,
};
use super::protocol::DecodeError;
use super::test_utils::{drawable, fill, memory, rect};
use super::transport::TransportError;

const RAM: u64 = 0x8000_0000;
const SLOT: u64 = 0x0101_0000_0000_0000;

struct Fixture {
    controller: Controller,
    sequences: [u32; 2],
}

impl Fixture {
    fn new() -> Self {
        let (memory, _) = memory();
        let mut fixture = Self {
            controller: Controller::new(memory, RAM, RAM + (32 << 20)).unwrap(),
            sequences: [0; 2],
        };
        fixture.slot();
        fixture.primary(SLOT | 64, 20, false);
        fixture
    }

    fn store(&self, offset: usize, data: &[u8]) {
        self.controller
            .memory()
            .write(Region::Ram, offset, data)
            .unwrap();
    }

    fn slot(&mut self) {
        self.store(MEMORY_SLOT, &RAM.to_le_bytes());
        self.store(MEMORY_SLOT + 8, &(RAM + RAM_HEADER as u64).to_le_bytes());
        self.controller.io_write(8, 1, 0).unwrap();
    }

    fn primary(&mut self, address: u64, stride: i32, keep: bool) {
        let mut data = [0; 40];
        for (offset, value) in [(0, 4u32), (4, 2), (12, 32), (24, u32::from(keep))] {
            data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        data[8..12].copy_from_slice(&stride.to_le_bytes());
        data[32..40].copy_from_slice(&address.to_le_bytes());
        self.store(CREATE_SURFACE, &data);
        self.controller.io_write(12, 0, 0).unwrap();
    }

    fn submit(&mut self, kind: u32, data: &[u8]) {
        let cursor = usize::from(kind == 3);
        let sequence = self.sequences[cursor];
        let address = 4096 + cursor * 16384 + sequence as usize * 256;
        let mut data = data.to_vec();
        data[..8].copy_from_slice(&(100 + address as u64).to_le_bytes());
        self.store(address, &data);
        let mut command = [0; 16];
        command[..8].copy_from_slice(&(SLOT | address as u64).to_le_bytes());
        command[8..12].copy_from_slice(&kind.to_le_bytes());
        let ring = if cursor == 0 {
            COMMAND_RING
        } else {
            CURSOR_RING
        };
        self.store(ring + 20 + (sequence & 31) as usize * 16, &command);
        self.sequences[cursor] += 1;
        self.controller
            .memory()
            .store(ring + 4, sequence + 1)
            .unwrap();
    }

    fn draw(&mut self, color: u32) {
        self.submit(
            1,
            &drawable(Layout::Modern, 1, [0, 0, 4, 2], &fill(color, 8), 0),
        );
    }

    fn word(&self, region: Region, offset: usize) -> u32 {
        let mut data = [0; 4];
        self.controller
            .memory()
            .read(region, offset, &mut data)
            .unwrap();
        u32::from_le_bytes(data)
    }
}

#[test]
fn synchronous_updates_complete_a_full_dispatch_before_returning() {
    let mut fixture = Fixture::new();
    for color in 1..=32 {
        fixture.draw(color);
    }
    fixture.store(UPDATE_AREA, &rect([0, 0, 4, 2]));
    fixture.controller.io_write(2, 0, 10).unwrap();
    assert_eq!(
        fixture.controller.frame(10).unwrap().unwrap().pixels,
        [32; 8]
    );
    assert_eq!(fixture.word(Region::Ram, COMMAND_RING + 12), 32);
    assert_eq!(fixture.word(Region::Ram, RELEASE_RING + 4), 1);
}

#[test]
fn update_commands_publish_ids_and_respect_interrupt_masking() {
    let mut fixture = Fixture::new();
    fixture.draw(7);
    let mut update = vec![0; 8];
    update.extend(rect([0, 0, 4, 2]));
    update.extend(42u32.to_le_bytes());
    update.extend(0u32.to_le_bytes());
    fixture.submit(2, &update);
    assert!(!fixture.controller.poll(12).unwrap());
    assert_eq!(fixture.word(Region::Rom, 8), 42);
    assert_eq!(fixture.word(Region::Rom, 48), 12);
    assert!(!fixture.controller.irq_level().unwrap());
    fixture
        .controller
        .memory()
        .store(RAM_HEADER + 8, 1)
        .unwrap();
    assert!(fixture.controller.irq_level().unwrap());
    fixture
        .controller
        .memory()
        .store(RAM_HEADER + 4, 0)
        .unwrap();
    fixture.controller.io_write(3, 0, 12).unwrap();
    assert!(!fixture.controller.irq_level().unwrap());
}

#[test]
fn release_backpressure_requires_more_polling_and_resumes_without_duplicates() {
    let mut fixture = Fixture::new();
    for color in 1..=8 {
        fixture.draw(color);
        assert!(!fixture.controller.poll(u64::from(color)).unwrap());
    }
    fixture.draw(9);
    assert!(fixture.controller.poll(10).unwrap());
    assert_eq!(
        fixture.controller.frame(10).unwrap().unwrap().pixels,
        [8; 8]
    );
    fixture
        .controller
        .memory()
        .store(RELEASE_RING + 12, 4)
        .unwrap();
    assert!(!fixture.controller.poll(11).unwrap());
    assert_eq!(
        fixture.controller.frame(11).unwrap().unwrap().pixels,
        [9; 8]
    );
}

#[test]
fn synchronous_update_defers_release_publication_without_waiting_for_the_guest() {
    let mut fixture = Fixture::new();
    for color in 1..=8 {
        fixture.draw(color);
        fixture.controller.poll(u64::from(color)).unwrap();
    }
    fixture.draw(9);
    fixture.controller.io_write(2, 0, 10).unwrap();
    assert_eq!(
        fixture.controller.frame(10).unwrap().unwrap().pixels,
        [9; 8]
    );
    assert_eq!(fixture.word(Region::Ram, RELEASE_RING + 4), 8);
    assert!(fixture.controller.poll(11).unwrap());
    fixture
        .controller
        .memory()
        .store(RELEASE_RING + 12, 4)
        .unwrap();
    assert!(!fixture.controller.poll(12).unwrap());
    assert_eq!(fixture.word(Region::Ram, RELEASE_RING + 4), 9);
}

#[test]
fn primary_transitions_preserve_padding_and_invalidate_stale_slots() {
    let mut fixture = Fixture::new();
    fixture.store(64, &[0xa5; 40]);
    fixture.primary(SLOT | 64, -20, false);
    assert_eq!(fixture.controller.frame(0).unwrap().unwrap().pixels, [0; 8]);
    assert_eq!(fixture.word(Region::Ram, 80), 0xa5a5a5a5);
    fixture.store(64, &7u32.to_le_bytes());
    fixture.primary(SLOT | 64, 20, true);
    assert_eq!(fixture.controller.frame(0).unwrap().unwrap().pixels[0], 7);
    fixture.controller.io_write(10, 0, 0).unwrap();
    assert!(fixture.controller.frame(0).unwrap().is_none());
    fixture.controller.io_write(11, 0, 0).unwrap();
    assert!(fixture.controller.frame(0).unwrap().is_some());
    let mut data = [0; 40];
    fixture
        .controller
        .memory()
        .read(Region::Ram, CREATE_SURFACE, &mut data)
        .unwrap();
    fixture.controller.io_write(5, 0, 0).unwrap();
    assert!(fixture.controller.frame(0).unwrap().is_none());
    fixture.slot();
    assert_eq!(fixture.word(Region::Rom, 68) & 255, 2);
    fixture.store(CREATE_SURFACE, &data);
    assert!(matches!(
        fixture.controller.io_write(12, 0, 0).unwrap_err(),
        TransportError::Decode(DecodeError::Slot)
    ));
    fixture.primary(0x0102_0000_0000_0040, 20, true);
    data[0..4].copy_from_slice(&0u32.to_le_bytes());
    fixture.store(CREATE_SURFACE, &data);
    fixture.controller.io_write(12, 0, 0).unwrap_err();
    assert_eq!(fixture.controller.frame(0).unwrap().unwrap().pixels[0], 7);
    fixture
        .controller
        .memory()
        .store(UPDATE_SURFACE, 1)
        .unwrap();
    fixture.controller.io_write(14, 0, 0).unwrap_err();
    assert!(fixture.controller.frame(0).unwrap().is_some());
    fixture
        .controller
        .memory()
        .store(UPDATE_SURFACE, 0)
        .unwrap();
    fixture.controller.io_write(14, 0, 0).unwrap();
    assert!(fixture.controller.frame(0).unwrap().is_none());
}

#[test]
fn cursor_commands_show_hide_move_and_expire_sampled_trails_without_burning_pixels() {
    let mut fixture = Fixture::new();
    let mut shape = vec![0; 42];
    for offset in [10, 12] {
        shape[offset..offset + 2].copy_from_slice(&1u16.to_le_bytes());
    }
    for offset in [18, 22] {
        shape[offset..offset + 4].copy_from_slice(&4u32.to_le_bytes());
    }
    shape.extend(u32::MAX.to_le_bytes());
    fixture.store(32768, &shape);
    let mut set = vec![0; 22];
    set[13] = 1;
    set[14..22].copy_from_slice(&(SLOT | 32768).to_le_bytes());
    fixture.submit(3, &set);
    fixture.controller.io_write(1, 0, 0).unwrap();
    assert_eq!(
        fixture.controller.frame(0).unwrap().unwrap().pixels[0],
        0xffffff
    );
    let mut command = vec![0; 13];
    command[8] = 3;
    command[9..13].copy_from_slice(&[2, 0, 10, 0]);
    fixture.submit(3, &command);
    fixture.controller.io_write(1, 0, 0).unwrap();
    for (x, now) in [(1, 10), (2, 20)] {
        command[8] = 1;
        command[9..13].copy_from_slice(&[x, 0, 0, 0]);
        fixture.submit(3, &command);
        fixture.controller.io_write(1, 0, now).unwrap();
    }
    assert_eq!(
        &fixture.controller.frame(20).unwrap().unwrap().pixels[..4],
        &[0xffffff, 0xffffff, 0xffffff, 0]
    );
    assert_eq!(
        &fixture.controller.frame(50).unwrap().unwrap().pixels[..4],
        &[0, 0, 0xffffff, 0]
    );
    assert_eq!(fixture.word(Region::Ram, 64), 0);
    command[8] = 2;
    fixture.submit(3, &command);
    fixture.controller.io_write(1, 0, 50).unwrap();
    assert_eq!(
        fixture.controller.frame(50).unwrap().unwrap().pixels,
        [0; 8]
    );
    command[8] = 1;
    fixture.submit(3, &command);
    fixture.controller.io_write(1, 0, 51).unwrap();
    assert_eq!(
        fixture.controller.frame(51).unwrap().unwrap().pixels[2],
        0xffffff
    );
    command[8] = 3;
    command[9..13].copy_from_slice(&[65, 0, 1, 0]);
    fixture.submit(3, &command);
    fixture.controller.io_write(1, 0, 51).unwrap_err();
    assert_eq!(
        &fixture.controller.frame(1).unwrap().unwrap().pixels[..4],
        &[0, 0, 0xffffff, 0]
    );
}
