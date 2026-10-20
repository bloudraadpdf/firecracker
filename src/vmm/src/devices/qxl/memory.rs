// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};

use vm_memory::{Bytes, MmapRegion, VolatileMemory};

use super::protocol::{Command, Ring, RingSize};
use super::state::MemoryState;

pub const SURFACE_BYTES: usize = 16 << 20;
pub const RAM_BYTES: usize = 32 << 20;
pub const VRAM_BYTES: usize = 32 << 20;
pub const ROM_BYTES: usize = 8192;
pub const RAM_HEADER: usize = RAM_BYTES - 8192;
pub const COMMAND_RING: usize = RAM_HEADER + 4108;
pub const CURSOR_RING: usize = COMMAND_RING + Ring::HEADER_SIZE + 32 * Command::SIZE;
pub const RELEASE_RING: usize = CURSOR_RING + Ring::HEADER_SIZE + 32 * Command::SIZE;
pub const UPDATE_AREA: usize = RELEASE_RING + Ring::HEADER_SIZE + 8 * 8;
pub const UPDATE_SURFACE: usize = UPDATE_AREA + 16;
pub const MEMORY_SLOT: usize = UPDATE_SURFACE + 4;
pub const CREATE_SURFACE: usize = MEMORY_SLOT + 16;
pub const FLAGS: usize = CREATE_SURFACE + 40;
pub const MONITORS_CONFIG: usize = FLAGS + 8;
const MODES_OFFSET: usize = 4096;

pub const MODES: &[(u32, u32)] = &[
    (640, 480),
    (800, 600),
    (1024, 768),
    (1280, 1024),
    (1600, 1200),
    (1152, 864),
    (1280, 768),
    (1280, 800),
    (1280, 960),
    (1440, 900),
    (1400, 1050),
    (1680, 1050),
    (1920, 1200),
    (1280, 720),
    (1920, 1080),
    (1600, 900),
    (1366, 768),
    (2560, 1440),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    Ram,
    Vram,
    Rom,
}

#[derive(Debug)]
pub struct Memory {
    boot: Arc<MmapRegion>,
    ram: Arc<MmapRegion>,
    vram: Arc<MmapRegion>,
    rom: Arc<MmapRegion>,
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "invalid QXL memory access")
}

impl Memory {
    pub(super) fn save(&self) -> io::Result<MemoryState> {
        let read = |region, offset, size| {
            let mut bytes = vec![0; size];
            self.read(region, offset, &mut bytes)?;
            Ok::<_, io::Error>(bytes)
        };
        Ok(MemoryState {
            ram: read(Region::Ram, SURFACE_BYTES, RAM_BYTES - SURFACE_BYTES)?,
            vram: read(Region::Vram, 0, VRAM_BYTES)?,
            rom: read(Region::Rom, 0, ROM_BYTES)?,
        })
    }

    pub(super) fn restore(boot: Arc<MmapRegion>, state: &MemoryState) -> io::Result<Self> {
        if state.ram.len() != RAM_BYTES - SURFACE_BYTES
            || state.vram.len() != VRAM_BYTES
            || state.rom.len() != ROM_BYTES
        {
            return Err(invalid());
        }
        let memory = Self::new(boot)?;
        let mut expected = vec![0; ROM_BYTES];
        memory.read(Region::Rom, 0, &mut expected)?;
        for range in [8..12, 20..24, 48..52, 68..69] {
            expected[range.clone()].copy_from_slice(&state.rom[range]);
        }
        let mode = u32::from_le_bytes(state.rom[20..24].try_into().unwrap());
        if expected != state.rom || (mode != u32::MAX && mode as usize >= MODES.len()) {
            return Err(invalid());
        }
        memory.write(Region::Ram, SURFACE_BYTES, &state.ram)?;
        memory.write(Region::Vram, 0, &state.vram)?;
        memory
            .rom
            .get_slice(0, ROM_BYTES)
            .map_err(io::Error::other)?
            .write_slice(&state.rom, 0)
            .map_err(io::Error::other)?;
        Ok(memory)
    }

    pub fn new(boot: Arc<MmapRegion>) -> io::Result<Self> {
        if boot.size() != SURFACE_BYTES {
            return Err(invalid());
        }
        let allocate = |size| {
            MmapRegion::new(size)
                .map(Arc::new)
                .map_err(io::Error::other)
        };
        let memory = Self {
            boot,
            ram: allocate(RAM_BYTES - SURFACE_BYTES)?,
            vram: allocate(VRAM_BYTES)?,
            rom: allocate(ROM_BYTES)?,
        };
        memory.reset(0)?;
        Ok(memory)
    }

    pub fn regions(&self) -> [(Region, usize, Arc<MmapRegion>); 4] {
        [
            (Region::Ram, 0, self.boot.clone()),
            (Region::Ram, SURFACE_BYTES, self.ram.clone()),
            (Region::Vram, 0, self.vram.clone()),
            (Region::Rom, 0, self.rom.clone()),
        ]
    }

    pub fn size(region: Region) -> usize {
        match region {
            Region::Ram => RAM_BYTES,
            Region::Vram => VRAM_BYTES,
            Region::Rom => ROM_BYTES,
        }
    }

    fn bank(&self, region: Region, offset: usize) -> (&MmapRegion, usize) {
        match region {
            Region::Ram if offset < SURFACE_BYTES => (&self.boot, offset),
            Region::Ram => (&self.ram, offset - SURFACE_BYTES),
            Region::Vram => (&self.vram, offset),
            Region::Rom => (&self.rom, offset),
        }
    }

    fn validate(region: Region, offset: usize, length: usize) -> io::Result<()> {
        if offset
            .checked_add(length)
            .is_none_or(|end| end > Self::size(region))
        {
            return Err(invalid());
        }
        Ok(())
    }

    pub fn read(&self, region: Region, offset: usize, output: &mut [u8]) -> io::Result<()> {
        Self::validate(region, offset, output.len())?;
        let mut copied = 0;
        while copied < output.len() {
            let (bank, position) = self.bank(region, offset + copied);
            let count = (bank.size() - position).min(output.len() - copied);
            bank.get_slice(position, count)
                .map_err(io::Error::other)?
                .read_slice(&mut output[copied..copied + count], 0)
                .map_err(io::Error::other)?;
            copied += count;
        }
        Ok(())
    }

    pub fn write(&self, region: Region, offset: usize, input: &[u8]) -> io::Result<()> {
        Self::validate(region, offset, input.len())?;
        if region == Region::Rom {
            return Err(invalid());
        }
        let mut copied = 0;
        while copied < input.len() {
            let (bank, position) = self.bank(region, offset + copied);
            let count = (bank.size() - position).min(input.len() - copied);
            bank.get_slice(position, count)
                .map_err(io::Error::other)?
                .write_slice(&input[copied..copied + count], 0)
                .map_err(io::Error::other)?;
            copied += count;
        }
        Ok(())
    }

    fn atomic(&self, region: Region, offset: usize) -> io::Result<&AtomicU32> {
        Self::validate(region, offset, 4)?;
        let (bank, position) = self.bank(region, offset);
        bank.get_atomic_ref::<AtomicU32>(position)
            .map_err(io::Error::other)
    }

    pub fn load(&self, offset: usize) -> io::Result<u32> {
        Ok(u32::from_le(
            self.atomic(Region::Ram, offset)?.load(Ordering::Acquire),
        ))
    }

    pub fn store(&self, offset: usize, value: u32) -> io::Result<()> {
        self.atomic(Region::Ram, offset)?
            .store(value.to_le(), Ordering::Release);
        Ok(())
    }

    pub fn interrupt(&self, bits: u32) -> io::Result<()> {
        self.atomic(Region::Ram, RAM_HEADER + 4)?
            .fetch_or(bits.to_le(), Ordering::AcqRel);
        Ok(())
    }

    pub(super) fn set_rom_word(&self, offset: usize, value: u32) -> io::Result<()> {
        self.atomic(Region::Rom, offset)?
            .store(value.to_le(), Ordering::Release);
        Ok(())
    }

    pub(super) fn set_generation(&self, generation: u8) -> io::Result<()> {
        self.rom
            .get_atomic_ref::<AtomicU8>(68)
            .map_err(io::Error::other)?
            .store(generation, Ordering::Release);
        Ok(())
    }

    pub fn reset(&self, generation: u8) -> io::Result<()> {
        let mut rom = [0; ROM_BYTES];
        rom[..4].copy_from_slice(b"QXRO");
        for (offset, value) in [
            (20, u32::MAX),
            (24, u32::try_from(MODES_OFFSET).unwrap()),
            (
                28,
                u32::try_from((RAM_HEADER - SURFACE_BYTES) / 4096).unwrap(),
            ),
            (32, u32::try_from(SURFACE_BYTES).unwrap()),
            (36, 0),
            (40, u32::try_from(SURFACE_BYTES).unwrap()),
            (44, u32::try_from(RAM_HEADER).unwrap()),
            (52, 1),
            (MODES_OFFSET, u32::try_from(MODES.len()).unwrap()),
        ] {
            rom[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        rom[64..70].copy_from_slice(&[1, 3, 8, 8, generation, 1]);
        for (index, &(width, height)) in MODES.iter().enumerate() {
            let start = MODES_OFFSET + 4 + index * 32;
            let values = [
                u32::try_from(index).unwrap(),
                width,
                height,
                32,
                width * 4,
                width * 254 / 960,
                height * 254 / 960,
                0,
            ];
            for (position, value) in values.into_iter().enumerate() {
                rom[start + position * 4..start + position * 4 + 4]
                    .copy_from_slice(&value.to_le_bytes());
            }
        }
        self.rom
            .get_slice(0, ROM_BYTES)
            .map_err(io::Error::other)?
            .write_slice(&rom, 0)
            .map_err(io::Error::other)?;
        self.write(Region::Ram, RAM_HEADER, &[0; 8192])?;
        self.write(Region::Ram, RAM_HEADER, b"QXRA")?;
        for (ring, size) in [
            (COMMAND_RING, RingSize::Commands),
            (CURSOR_RING, RingSize::Commands),
            (RELEASE_RING, RingSize::Releases),
        ] {
            self.store(ring, size.capacity())?;
            self.store(ring + 8, 1)?;
        }
        Ok(())
    }
}
