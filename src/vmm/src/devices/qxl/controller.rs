// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeMap;

use super::cursor::{Command as CursorCommand, Pointer};
use super::drawing::{Drawable, Layout};
use super::memory::{
    CREATE_SURFACE, MEMORY_SLOT, MODES, Memory, RAM_HEADER, Region, UPDATE_AREA, UPDATE_SURFACE,
};
use super::protocol::{
    Command, CommandKind, DecodeError, MAX_SURFACES, PrimarySurface, Rect, bytes,
};
use super::state::ControllerState;
use super::surface::Surface;
use super::transport::{AddressSpace, Queue, Queues, TransportError};
use crate::devices::display::Frame;

#[derive(Debug)]
pub struct Controller {
    memory: Memory,
    addresses: AddressSpace,
    queues: Queues,
    primary: Option<PrimarySurface>,
    // Pixels remain in validated guest banks; only live descriptors need tracking.
    surfaces: BTreeMap<u32, PrimarySurface>,
    attached: bool,
    pointer: Pointer,
}

impl Controller {
    /// Capture private memory and host metadata while the guest is paused.
    pub fn save(&self) -> Result<ControllerState, TransportError> {
        Ok(ControllerState {
            memory: self.memory.save()?,
            addresses: self.addresses.save(),
            queues: self.queues.save(),
            primary: self.primary.map(|primary| primary.encode().to_vec()),
            surfaces: self
                .surfaces
                .iter()
                .map(|(id, surface)| (*id, surface.encode().to_vec()))
                .collect(),
            attached: self.attached,
            pointer: self.pointer.save(),
        })
    }

    /// Validate checkpoint metadata before it can reach drawing or queue execution.
    pub fn restore(
        boot: std::sync::Arc<vm_memory::MmapRegion>,
        state: &ControllerState,
        ram: u64,
        vram: u64,
    ) -> Result<Self, TransportError> {
        if (state.attached && state.primary.is_none())
            || state
                .primary
                .as_ref()
                .is_some_and(|primary| primary.len() != PrimarySurface::SIZE)
        {
            return Err(DecodeError::Parameter.into());
        }
        if state.surfaces.len() >= MAX_SURFACES as usize {
            return Err(DecodeError::Parameter.into());
        }
        let pointer = Pointer::restore(&state.pointer)?;
        let addresses = AddressSpace::restore(&state.addresses, ram, vram)?;
        let memory = Memory::restore(boot, &state.memory)?;
        let queues = Queues::restore(&state.queues, &memory, &addresses)?;
        let primary = state
            .primary
            .as_ref()
            .map(|primary| PrimarySurface::decode(primary))
            .transpose()?;
        if let Some(primary) = primary {
            Surface::open(&memory, &addresses, primary)?;
        }
        let mut surfaces = BTreeMap::new();
        for (id, data) in &state.surfaces {
            if *id == 0
                || *id >= MAX_SURFACES
                || data.len() != PrimarySurface::SIZE
                || surfaces.contains_key(id)
            {
                return Err(DecodeError::Parameter.into());
            }
            let surface = PrimarySurface::decode(data)?;
            Surface::open(&memory, &addresses, surface)?;
            surfaces.insert(*id, surface);
        }
        Ok(Self {
            memory,
            addresses,
            queues,
            primary,
            surfaces,
            attached: state.attached,
            pointer,
        })
    }

    pub fn new(memory: Memory, ram: u64, vram: u64) -> Result<Self, TransportError> {
        Ok(Self {
            memory,
            addresses: AddressSpace::new(ram, vram)?,
            queues: Queues::default(),
            primary: None,
            surfaces: BTreeMap::new(),
            attached: false,
            pointer: Pointer::default(),
        })
    }

    pub fn memory(&self) -> &Memory {
        &self.memory
    }

    pub fn relocate(&mut self, ram: u64, vram: u64) -> Result<(), TransportError> {
        Ok(self.addresses.relocate(ram, vram)?)
    }

    pub fn irq_level(&self) -> Result<bool, TransportError> {
        Ok(self.memory.load(RAM_HEADER + 4)? & self.memory.load(RAM_HEADER + 8)? != 0)
    }

    fn surface<'a>(
        memory: &'a Memory,
        addresses: &AddressSpace,
        primary: Option<PrimarySurface>,
    ) -> Result<Surface<'a>, TransportError> {
        Surface::open(memory, addresses, primary.ok_or(DecodeError::Parameter)?)
    }

    fn execute(
        memory: &Memory,
        addresses: &AddressSpace,
        primary: Option<PrimarySurface>,
        surfaces: &mut BTreeMap<u32, PrimarySurface>,
        pointer: &mut Pointer,
        command: Command,
        now: u64,
    ) -> Result<(), TransportError> {
        match command.kind {
            CommandKind::Draw => {
                let descriptor = |id| {
                    if id == 0 {
                        primary
                    } else {
                        surfaces.get(&id).copied()
                    }
                };
                let resolve = |id| Self::surface(memory, addresses, descriptor(id))?.snapshot();
                let draw = Drawable::decode_with_surfaces(
                    memory,
                    addresses,
                    command.address,
                    Layout::Modern,
                    Some(&resolve),
                )?;
                draw.render(&Self::surface(
                    memory,
                    addresses,
                    descriptor(draw.surface_id()),
                )?)?;
            }
            CommandKind::Update => {
                let mut data = [0; 32];
                addresses.read(memory, command.address, &mut data)?;
                let id = u32::from_le_bytes(bytes(&data, 28)?);
                let descriptor = if id == 0 {
                    primary
                } else {
                    surfaces.get(&id).copied()
                };
                let surface = Self::surface(memory, addresses, descriptor)?;
                let (width, height) = surface.dimensions();
                Rect::decode(&data[8..24])?.clipped(width, height)?;
                memory.set_rom_word(8, u32::from_le_bytes(bytes(&data, 24)?))?;
                memory.interrupt(1)?;
            }
            CommandKind::Cursor => pointer.apply(
                CursorCommand::decode(memory, addresses, command.address)?,
                now,
            )?,
            CommandKind::Nop | CommandKind::Message => (),
            CommandKind::Surface => {
                let mut header = [0; 17];
                addresses.read(memory, command.address, &mut header)?;
                let id = u32::from_le_bytes(bytes(&header, 8)?);
                let flags = u32::from_le_bytes(bytes(&header, 13)?);
                if id == 0 || id >= MAX_SURFACES || flags & !1 != 0 {
                    return Err(DecodeError::Parameter.into());
                }
                match header[12] {
                    0 => {
                        if surfaces.contains_key(&id) {
                            return Err(DecodeError::Parameter.into());
                        }
                        let mut data = [0; 41];
                        addresses.read(memory, command.address, &mut data)?;
                        let mut descriptor = [0; PrimarySurface::SIZE];
                        descriptor[0..12].copy_from_slice(&data[21..33]);
                        descriptor[12..16].copy_from_slice(&data[17..21]);
                        descriptor[24..28].copy_from_slice(&flags.to_le_bytes());
                        descriptor[32..40].copy_from_slice(&data[33..41]);
                        let descriptor = PrimarySurface::decode(&descriptor)?;
                        let surface = Surface::open(memory, addresses, descriptor)?;
                        if !descriptor.keep_data() {
                            surface.clear()?;
                        }
                        surfaces.insert(id, descriptor);
                    }
                    1 if flags == 0 => {
                        surfaces.remove(&id).ok_or(DecodeError::Parameter)?;
                    }
                    _ => return Err(DecodeError::Parameter.into()),
                }
            }
        }
        Ok(())
    }

    fn clock(&self, now: u64) -> Result<(), TransportError> {
        self.memory
            .set_rom_word(48, u32::try_from(now & u64::from(u32::MAX)).unwrap())?;
        Ok(())
    }

    fn dispatch(
        &mut self,
        queue: Queue,
        now: u64,
        synchronous: bool,
    ) -> Result<(), TransportError> {
        let execute = |command| {
            Self::execute(
                &self.memory,
                &self.addresses,
                self.primary,
                &mut self.surfaces,
                &mut self.pointer,
                command,
                now,
            )
        };
        if synchronous {
            self.queues
                .synchronize(&self.memory, &self.addresses, queue, execute)?;
        } else {
            self.queues
                .drain(&self.memory, &self.addresses, queue, execute)?;
        }
        Ok(())
    }

    /// Process bounded work; true requests another poll, including after release backpressure.
    pub fn poll(&mut self, now: u64) -> Result<bool, TransportError> {
        self.clock(now)?;
        for queue in [Queue::Commands, Queue::Cursor] {
            self.dispatch(queue, now, false)?;
        }
        Ok(self.queues.pending(&self.memory, Queue::Commands)?
            || self.queues.pending(&self.memory, Queue::Cursor)?
            || self.queues.pending_releases())
    }

    fn synchronize(&mut self, now: u64) -> Result<(), TransportError> {
        self.clock(now)?;
        for queue in [Queue::Commands, Queue::Cursor] {
            self.dispatch(queue, now, true)?;
        }
        Ok(())
    }

    pub fn io_write(&mut self, port: u8, value: u32, now: u64) -> Result<(), TransportError> {
        match port {
            0 | 1 | 4 => {
                self.poll(now)?;
            }
            2 => {
                self.synchronize(now)?;
                let id = self.memory.load(UPDATE_SURFACE)?;
                let descriptor = if id == 0 {
                    self.primary
                } else {
                    self.surfaces.get(&id).copied()
                };
                let mut data = [0; 16];
                self.memory.read(Region::Ram, UPDATE_AREA, &mut data)?;
                let surface = Self::surface(&self.memory, &self.addresses, descriptor)?;
                let (width, height) = surface.dimensions();
                Rect::decode(&data)?.clipped(width, height)?;
            }
            3 | 7 => (),
            5 => {
                self.memory.reset(0)?;
                self.addresses.reset();
                self.surfaces.clear();
                self.queues = Queues::default();
                self.primary = None;
                self.attached = false;
                self.pointer = Pointer::default();
            }
            6 => {
                if !usize::try_from(value).is_ok_and(|index| index < MODES.len()) {
                    return Err(DecodeError::Parameter.into());
                }
                self.synchronize(now)?;
                self.memory.set_rom_word(20, value)?;
                self.primary = None;
                self.attached = false;
            }
            8 => {
                let mut data = [0; 16];
                self.memory.read(Region::Ram, MEMORY_SLOT, &mut data)?;
                let start = u64::from_le_bytes(bytes(&data, 0)?);
                let end = u64::from_le_bytes(bytes(&data, 8)?);
                let generation = self.addresses.add(
                    u8::try_from(value).map_err(|_| DecodeError::Slot)?,
                    start..end,
                )?;
                self.memory.set_generation(generation)?;
            }
            9 => {
                self.synchronize(now)?;
                if self.queues.pending_releases() {
                    return Err(TransportError::Busy);
                }
                self.addresses
                    .remove(u8::try_from(value).map_err(|_| DecodeError::Slot)?)?;
                self.surfaces
                    .retain(|_, surface| surface.address() >> 56 != u64::from(value));
                if self
                    .primary
                    .is_some_and(|primary| primary.address() >> 56 == u64::from(value))
                {
                    self.primary = None;
                    self.attached = false;
                }
            }
            10 => self.attached = false,
            11 => {
                if self.primary.is_none() {
                    return Err(DecodeError::Parameter.into());
                }
                self.attached = true;
            }
            12 => {
                self.synchronize(now)?;
                let mut data = [0; 40];
                self.memory.read(Region::Ram, CREATE_SURFACE, &mut data)?;
                let primary = PrimarySurface::decode(&data)?;
                let surface = Surface::open(&self.memory, &self.addresses, primary)?;
                if !primary.keep_data() {
                    surface.clear()?;
                }
                self.primary = Some(primary);
                self.attached = true;
            }
            13..=15 => {
                self.synchronize(now)?;
                if port == 14 {
                    let id = value;
                    if id == 0 {
                        self.primary = None;
                        self.attached = false;
                    } else {
                        self.surfaces.remove(&id).ok_or(DecodeError::Parameter)?;
                    }
                } else {
                    self.primary = None;
                    self.attached = false;
                    if port == 15 {
                        self.surfaces.clear();
                    }
                }
            }
            other => return Err(DecodeError::Command(u32::from(other)).into()),
        }
        Ok(())
    }

    pub fn frame(&mut self, now: u64) -> Result<Option<Frame>, TransportError> {
        if !self.attached {
            return Ok(None);
        }
        let mut frame = Self::surface(&self.memory, &self.addresses, self.primary)?.frame()?;
        self.pointer.draw(&mut frame, now);
        Ok(Some(frame))
    }
}
