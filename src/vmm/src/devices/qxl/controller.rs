// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use super::cursor::{Command as CursorCommand, Pointer};
use super::drawing::{Drawable, Layout};
use super::memory::{
    CREATE_SURFACE, MEMORY_SLOT, MODES, Memory, RAM_HEADER, Region, UPDATE_AREA, UPDATE_SURFACE,
};
use super::protocol::{Command, CommandKind, DecodeError, PrimarySurface, Rect, bytes};
use super::surface::Surface;
use super::transport::{AddressSpace, Queue, Queues, TransportError};
use crate::devices::display::Frame;

#[derive(Debug)]
pub struct Controller {
    memory: Memory,
    addresses: AddressSpace,
    queues: Queues,
    primary: Option<PrimarySurface>,
    attached: bool,
    pointer: Pointer,
}

impl Controller {
    pub fn new(memory: Memory, ram: u64, vram: u64) -> Result<Self, TransportError> {
        Ok(Self {
            memory,
            addresses: AddressSpace::new(ram, vram)?,
            queues: Queues::default(),
            primary: None,
            attached: false,
            pointer: Pointer::default(),
        })
    }

    pub fn memory(&self) -> &Memory {
        &self.memory
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
        pointer: &mut Pointer,
        command: Command,
        now: u64,
    ) -> Result<(), TransportError> {
        match command.kind {
            CommandKind::Draw => {
                let draw = Drawable::decode(memory, addresses, command.address, Layout::Modern)?;
                if draw.surface_id() != 0 {
                    return Err(DecodeError::Parameter.into());
                }
                draw.render(&Self::surface(memory, addresses, primary)?)?;
            }
            CommandKind::Update => {
                let mut data = [0; 32];
                addresses.read(memory, command.address, &mut data)?;
                if u32::from_le_bytes(bytes(&data, 28)?) != 0 {
                    return Err(DecodeError::Parameter.into());
                }
                let surface = Self::surface(memory, addresses, primary)?;
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
            CommandKind::Surface => return Err(DecodeError::Command(5).into()),
        }
        Ok(())
    }

    /// Process bounded work; true requests another poll, including after release backpressure.
    pub fn poll(&mut self, now: u64) -> Result<bool, TransportError> {
        self.memory
            .set_rom_word(48, u32::try_from(now & u64::from(u32::MAX)).unwrap())?;
        for queue in [Queue::Commands, Queue::Cursor] {
            self.queues
                .drain(&self.memory, &self.addresses, queue, |command| {
                    Self::execute(
                        &self.memory,
                        &self.addresses,
                        self.primary,
                        &mut self.pointer,
                        command,
                        now,
                    )
                })?;
        }
        Ok(self.queues.pending(&self.memory, Queue::Commands)?
            || self.queues.pending(&self.memory, Queue::Cursor)?)
    }

    fn synchronize(&mut self, now: u64) -> Result<(), TransportError> {
        if self.poll(now)? {
            return Err(TransportError::Busy);
        }
        Ok(())
    }

    pub fn io_write(&mut self, port: u8, value: u8, now: u64) -> Result<(), TransportError> {
        match port {
            0 | 1 | 4 => {
                self.poll(now)?;
            }
            2 => {
                self.synchronize(now)?;
                if self.memory.load(UPDATE_SURFACE)? != 0 {
                    return Err(DecodeError::Parameter.into());
                }
                let mut data = [0; 16];
                self.memory.read(Region::Ram, UPDATE_AREA, &mut data)?;
                let surface = Self::surface(&self.memory, &self.addresses, self.primary)?;
                let (width, height) = surface.dimensions();
                Rect::decode(&data)?.clipped(width, height)?;
            }
            3 | 7 => (),
            5 => {
                self.memory.reset(0)?;
                self.addresses.reset();
                self.queues = Queues::default();
                self.primary = None;
                self.attached = false;
                self.pointer = Pointer::default();
            }
            6 => {
                if usize::from(value) >= MODES.len() {
                    return Err(DecodeError::Parameter.into());
                }
                self.synchronize(now)?;
                self.memory.set_rom_word(20, u32::from(value))?;
                self.primary = None;
                self.attached = false;
            }
            8 => {
                let mut data = [0; 16];
                self.memory.read(Region::Ram, MEMORY_SLOT, &mut data)?;
                let start = u64::from_le_bytes(bytes(&data, 0)?);
                let end = u64::from_le_bytes(bytes(&data, 8)?);
                let generation = self.addresses.add(value, start..end)?;
                self.memory.set_generation(generation)?;
            }
            9 => {
                self.synchronize(now)?;
                self.addresses.remove(value)?;
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
                if port == 14 && (value != 0 || self.memory.load(UPDATE_SURFACE)? != 0) {
                    return Err(DecodeError::Parameter.into());
                }
                self.synchronize(now)?;
                self.primary = None;
                self.attached = false;
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
