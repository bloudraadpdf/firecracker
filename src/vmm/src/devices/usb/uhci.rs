// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! UHCI 1.1 registers and bounded, synchronous guest-memory schedule execution.

use std::io;

use serde::{Deserialize, Serialize};
use vm_memory::GuestMemoryBackend;

use super::hid::{HidTablet, HidTabletState, UsbResult};
use crate::vstate::memory::{Bytes, GuestAddress, GuestMemoryMmap};

const RUN: u16 = 1;
const SUSPEND: u16 = 8;
const DEBUG: u16 = 32;
const USBINT: u16 = 1;
const USBERR: u16 = 2;
const HOST_ERROR: u16 = 8;
const PROCESS_ERROR: u16 = 16;
const HALTED: u16 = 32;
const ACTIVE: u32 = 1 << 23;
const STALL: u32 = 1 << 22;
const BABBLE: u32 = 1 << 20;
const NAK: u32 = 1 << 19;
const TIMEOUT: u32 = 1 << 18;
const IOC: u32 = 1 << 24;
const ISO: u32 = 1 << 25;
const LOW_SPEED: u32 = 1 << 26;
const SHORT: u32 = 1 << 29;
const MAX_NODES: usize = 256;
const FRAME_BYTES: usize = 1500;
const MAX_PACKET: usize = 1280;

#[derive(Clone, Copy)]
struct Transfer {
    address: u32,
    status: u32,
    token: u32,
    buffer: u32,
    length: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Cursor {
    link: u32,
    queue: Option<(u32, u32)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Registers {
    command: u16,
    status: u16,
    interrupt_enable: u16,
    interrupt_causes: u8,
    frame: u16,
    base: u32,
    sof: u8,
    ports: [u16; 2],
    debug_cursor: Option<Cursor>,
}

impl Default for Registers {
    fn default() -> Self {
        Self {
            command: 0,
            status: HALTED,
            interrupt_enable: 0,
            interrupt_causes: 0,
            frame: 0,
            base: 0,
            sof: 64,
            ports: [0x93, 0x80],
            debug_cursor: None,
        }
    }
}

/// Controller registers and USB device protocol state. DMA never outlives a tick.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UhciState {
    registers: Registers,
    tablet: HidTabletState,
}

/// Two root ports, with one permanently attached full-speed HID tablet.
#[derive(Debug)]
pub struct Uhci {
    registers: Registers,
    tablet: HidTablet,
}

impl Default for Uhci {
    fn default() -> Self {
        Self::new()
    }
}

impl Uhci {
    /// Construct a stopped controller with the tablet attached to port one.
    pub fn new() -> Self {
        Self {
            registers: Registers::default(),
            tablet: HidTablet::new(),
        }
    }

    /// Restore only a canonical controller state and validated HID state.
    pub fn restore(state: &UhciState) -> io::Result<Self> {
        let r = &state.registers;
        if r.command & !0xfd != 0
            || r.status & !0x3f != 0
            || r.interrupt_enable & !15 != 0
            || r.interrupt_causes & !3 != 0
            || (r.status & USBINT != 0) != (r.interrupt_causes != 0)
            || (r.status & HALTED != 0) != (r.command & RUN == 0)
            || r.frame > 2047
            || r.base & 4095 != 0
            || r.sof > 127
            || r.ports[0] & !0x124e != 0x91
            || r.ports[0] & 0x204 == 0x204
            || r.ports[1] & !0x1240 != 0x80
            || r.debug_cursor.as_ref().is_some_and(|c| {
                r.command & DEBUG == 0 || c.queue.is_some_and(|(address, _)| address & 15 != 0)
            })
        {
            return Err(io::Error::other("invalid UHCI snapshot registers"));
        }
        Ok(Self {
            registers: r.clone(),
            tablet: HidTablet::restore(&state.tablet)?,
        })
    }

    /// Capture registers and pending USB control and input state.
    pub fn save(&self) -> UhciState {
        UhciState {
            registers: self.registers.clone(),
            tablet: self.tablet.save(),
        }
    }

    /// Read the 32-byte PCI I/O register window.
    pub fn read(&self, offset: u64, data: &mut [u8]) {
        let mut bytes = [0xff; 32];
        let r = &self.registers;
        for (offset, value) in [
            (0, r.command),
            (2, r.status),
            (4, r.interrupt_enable),
            (6, r.frame),
            (16, r.ports[0]),
            (18, r.ports[1]),
        ] {
            bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        }
        bytes[8..12].copy_from_slice(&r.base.to_le_bytes());
        bytes[12] = r.sof;
        data.fill(0xff);
        if let Ok(offset) = usize::try_from(offset)
            && let Some(end) = offset.checked_add(data.len())
            && let Some(source) = bytes.get(offset..end)
        {
            data.copy_from_slice(source);
        }
    }

    /// Apply register writes; status and root-port changes are write-one-to-clear.
    pub fn write(&mut self, offset: u64, data: &[u8]) {
        let Ok(offset) = usize::try_from(offset) else {
            return;
        };
        if data.is_empty() || data.len() > 4 {
            return;
        }
        let mut bytes = [0; 4];
        bytes[..data.len()].copy_from_slice(data);
        let value = u32::from_le_bytes(bytes);
        if offset <= 5 && data.len() <= 2 && offset % 2 + data.len() <= 2 {
            let shift = (offset % 2) * 8;
            let mask = if data.len() == 2 {
                u16::MAX
            } else {
                255 << shift
            };
            let value = u16::try_from(value).unwrap() << shift;
            match offset / 2 {
                0 => {
                    let value = (self.registers.command & !mask) | value;
                    if value & 6 != 0 {
                        self.registers = Registers::default();
                        self.tablet.reset();
                        self.registers.command = value & 4;
                        return;
                    }
                    self.registers.command = value & 0xf9;
                    if value & RUN != 0 {
                        self.registers.status &= !HALTED;
                    } else {
                        self.registers.status |= HALTED;
                    }
                    if value & DEBUG == 0 {
                        self.registers.debug_cursor = None;
                    }
                }
                1 => {
                    self.registers.status &= !(value & 31);
                    if value & USBINT != 0 {
                        self.registers.interrupt_causes = 0;
                    }
                }
                2 => {
                    self.registers.interrupt_enable =
                        ((self.registers.interrupt_enable & !mask) | value) & 15
                }
                _ => unreachable!(),
            }
        } else if offset == 6 && data.len() == 2 && self.registers.command & RUN == 0 {
            self.registers.frame = u16::try_from(value & 2047).unwrap();
            self.registers.debug_cursor = None;
        } else if (8..12).contains(&offset) && offset + data.len() <= 12 {
            let mut base = self.registers.base.to_le_bytes();
            base[offset - 8..offset - 8 + data.len()].copy_from_slice(data);
            self.registers.base = u32::from_le_bytes(base) & !4095;
        } else if offset == 12 && data.len() == 1 {
            self.registers.sof = data[0] & 127;
        } else if matches!(offset, 16 | 18) && data.len() == 2 {
            let index = (offset - 16) / 2;
            let value = u16::try_from(value).unwrap();
            let old = self.registers.ports[index];
            self.registers.ports[index] = (old & !(0x1244 | (value & 10))) | (value & 0x1244);
            if value & 0x200 != 0 || index == 1 {
                self.registers.ports[index] &= !4;
                if index == 0 && old & 0x200 == 0 {
                    self.tablet.reset();
                }
            }
        }
    }

    /// Level-triggered interrupt state, before PCI command and legacy routing gates.
    pub fn interrupt_pending(&self) -> bool {
        let r = &self.registers;
        r.status & (HOST_ERROR | PROCESS_ERROR) != 0
            || r.status & USBERR != 0 && r.interrupt_enable & 1 != 0
            || r.status & 4 != 0 && r.interrupt_enable & 2 != 0
            || r.interrupt_causes & 1 != 0 && r.interrupt_enable & 4 != 0
            || r.interrupt_causes & 2 != 0 && r.interrupt_enable & 8 != 0
    }

    /// True once a guest has configured the tablet and enabled its root port.
    pub fn tablet_ready(&self) -> bool {
        self.tablet.configured()
            && self.registers.ports[0] & 0x1204 == 4
            && self.registers.command & (RUN | SUSPEND | 4) == RUN
    }

    /// Queue an absolute input report using the USB HID coordinate range.
    pub fn pointer(&mut self, x: u16, y: u16, buttons: u8, wheel: i8) -> io::Result<()> {
        if !self.tablet_ready() {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "USB tablet is not configured",
            ));
        }
        if x > 32767 || y > 32767 || buttons & !7 != 0 || wheel == i8::MIN {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid USB pointer report",
            ));
        }
        if !self.tablet.update(x, y, buttons, wheel) {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "USB pointer queue is full",
            ));
        }
        Ok(())
    }

    /// Release all tablet buttons on client disconnect.
    pub fn release(&mut self) -> io::Result<()> {
        if self.tablet.release() {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "USB pointer queue is full",
            ))
        }
    }

    fn fatal(&mut self, status: u16) {
        self.registers.status |= status | HALTED;
        self.registers.command &= !RUN;
        self.registers.debug_cursor = None;
    }

    fn completion(&mut self, ioc: bool, short: bool) {
        self.registers.interrupt_causes |= u8::from(ioc) | (u8::from(short) << 1);
        if self.registers.interrupt_causes != 0 {
            self.registers.status |= USBINT;
        }
    }

    /// Execute one millisecond. The caller gates PCI bus mastering and VM pause.
    pub fn tick(&mut self, memory: &GuestMemoryMmap) {
        self.tablet.frame_tick();
        if self.registers.command & (RUN | SUSPEND | 4) != RUN {
            return;
        }
        if self.execute_frame(memory).is_err() {
            self.fatal(HOST_ERROR);
        }
    }

    fn execute_frame(&mut self, memory: &GuestMemoryMmap) -> io::Result<()> {
        let mut cursor = if let Some(cursor) = self.registers.debug_cursor.take() {
            cursor
        } else {
            let address = self.registers.base + u32::from(self.registers.frame & 1023) * 4;
            Cursor {
                link: read_word(memory, address)?,
                queue: None,
            }
        };
        let mut budget = FRAME_BYTES;
        for _ in 0..MAX_NODES {
            if cursor.link & 1 != 0 {
                break;
            }
            if cursor.link & 8 != 0 {
                self.fatal(PROCESS_ERROR);
                return Ok(());
            }
            let address = cursor.link & !15;
            if cursor.link & 2 != 0 {
                let horizontal = read_word(memory, address)?;
                if horizontal & 12 != 0 {
                    self.fatal(PROCESS_ERROR);
                    return Ok(());
                }
                let element = read_word(memory, address + 4)?;
                if element & 1 != 0 {
                    cursor = Cursor {
                        link: horizontal,
                        queue: None,
                    };
                } else {
                    cursor = Cursor {
                        link: element,
                        queue: Some((address, horizontal)),
                    };
                }
                continue;
            }
            let mut descriptor = [0; 16];
            memory
                .read_slice(&mut descriptor, GuestAddress(u64::from(address)))
                .map_err(io::Error::other)?;
            let words = descriptor.as_chunks::<4>().0;
            let [link, status, token, buffer] =
                [words[0], words[1], words[2], words[3]].map(u32::from_le_bytes);
            let length = usize::try_from(((token >> 21) + 1) & 2047).unwrap();
            if status & ACTIVE != 0 && length > MAX_PACKET {
                self.fatal(PROCESS_ERROR);
                return Ok(());
            }
            if status & ACTIVE != 0 && length + 32 > budget {
                break;
            }
            let td = Transfer {
                address,
                status,
                token,
                buffer,
                length,
            };
            let (advance, stop_frame) = self.execute_td(memory, &td, cursor.queue.is_some())?;
            if self.registers.command & RUN == 0 {
                return Ok(());
            }
            if status & ACTIVE != 0 {
                budget -= length + 32;
            }
            if let Some((queue, horizontal)) = cursor.queue {
                if advance {
                    write_word(memory, queue + 4, link)?;
                    if link & 5 == 4 {
                        cursor.link = link;
                    } else {
                        cursor = Cursor {
                            link: horizontal,
                            queue: None,
                        };
                    }
                } else {
                    cursor = Cursor {
                        link: horizontal,
                        queue: None,
                    };
                }
            } else {
                cursor.link = link;
            }
            if stop_frame {
                break;
            }
            if self.registers.command & DEBUG != 0 {
                self.registers.command &= !RUN;
                self.registers.status |= HALTED;
                self.registers.debug_cursor = Some(cursor);
                return Ok(());
            }
        }
        self.registers.frame = (self.registers.frame + 1) & 2047;
        Ok(())
    }

    fn execute_td(
        &mut self,
        memory: &GuestMemoryMmap,
        td: &Transfer,
        queued: bool,
    ) -> io::Result<(bool, bool)> {
        let Transfer {
            address,
            status,
            token,
            buffer,
            length,
        } = *td;
        if status & ACTIVE == 0 {
            self.completion(status & IOC != 0, false);
            return Ok((false, false));
        }
        let pid = u8::try_from(token & 255).unwrap();
        if !matches!(pid, 0x2d | 0x69 | 0xe1) || length > MAX_PACKET {
            self.fatal(PROCESS_ERROR);
            return Ok((false, false));
        }
        if length != 0 && !memory.check_range(GuestAddress(u64::from(buffer)), length) {
            return Err(io::Error::other("UHCI DMA buffer is outside guest RAM"));
        }
        let mut outgoing = [0; MAX_PACKET];
        if pid != 0x69 && length != 0 {
            memory
                .read_slice(&mut outgoing[..length], GuestAddress(u64::from(buffer)))
                .map_err(io::Error::other)?;
        }
        let response = if self.registers.ports[0] & 0x1204 != 4 || status & LOW_SPEED != 0 {
            UsbResult::NoDevice
        } else {
            self.tablet.transaction(
                pid,
                u8::try_from((token >> 8) & 127).unwrap(),
                u8::try_from((token >> 15) & 15).unwrap(),
                token & (1 << 19) != 0,
                if pid == 0x69 {
                    &[]
                } else {
                    &outgoing[..length]
                },
                length,
            )
        };
        let mut next = status & !0x00fe_07ff;
        let mut advance = false;
        let mut short = false;
        match response {
            UsbResult::Ack { data, len } => {
                let actual = if pid == 0x69 {
                    usize::from(len)
                } else {
                    length
                };
                if actual > length || actual > data.len() && pid == 0x69 {
                    next |= STALL | BABBLE | 2047;
                    self.registers.status |= USBERR;
                } else {
                    if pid == 0x69 && actual != 0 {
                        memory
                            .write_slice(&data[..actual], GuestAddress(u64::from(buffer)))
                            .map_err(io::Error::other)?;
                    }
                    next |= u32::try_from(actual.wrapping_sub(1) & 2047).unwrap();
                    short = pid == 0x69 && actual < length && queued && status & SHORT != 0;
                    advance = !short;
                }
            }
            UsbResult::Nak => next |= ACTIVE | NAK | (status & 2047),
            UsbResult::Stall => {
                next |= STALL | 2047;
                if pid == 0x2d {
                    next |= TIMEOUT;
                }
                self.registers.status |= USBERR;
            }
            UsbResult::NoDevice => {
                let errors = (status >> 27) & 3;
                next |= ACTIVE | TIMEOUT | (status & 2047);
                if errors != 0 {
                    next = (next & !(3 << 27)) | ((errors - 1) << 27);
                    if errors == 1 {
                        next = (next & !ACTIVE) | STALL;
                        self.registers.status |= USBERR;
                    }
                }
            }
        }
        if status & ISO != 0 {
            next &= !ACTIVE;
        }
        write_word(memory, address + 4, next)?;
        self.completion(next & ACTIVE == 0 && status & IOC != 0, short);
        Ok((advance, next & BABBLE != 0))
    }
}

fn read_word(memory: &GuestMemoryMmap, address: u32) -> io::Result<u32> {
    let mut bytes = [0; 4];
    memory
        .read_slice(&mut bytes, GuestAddress(u64::from(address)))
        .map_err(io::Error::other)?;
    Ok(u32::from_le_bytes(bytes))
}

fn write_word(memory: &GuestMemoryMmap, address: u32, value: u32) -> io::Result<()> {
    memory
        .write_slice(&value.to_le_bytes(), GuestAddress(u64::from(address)))
        .map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vmm_config::machine_config::HugePageConfig;
    use crate::vstate::memory::{self, Bitmap, GuestMemoryExtension, GuestMemoryRegion};

    fn memory() -> GuestMemoryMmap {
        memory::test_utils::into_region_ext(
            memory::anonymous(
                [(GuestAddress(0), 0x10000)].into_iter(),
                true,
                HugePageConfig::None,
            )
            .unwrap(),
        )
    }

    fn put_td(mem: &GuestMemoryMmap, address: u32, words: [u32; 4]) {
        for (index, word) in words.into_iter().enumerate() {
            write_word(mem, address + u32::try_from(index * 4).unwrap(), word).unwrap();
        }
    }

    fn schedule(mem: &GuestMemoryMmap) -> Uhci {
        for frame in 0..1024 {
            write_word(mem, 0x1000 + frame * 4, 0x2002).unwrap();
        }
        write_word(mem, 0x2000, 1).unwrap();
        write_word(mem, 0x2004, 0x3000).unwrap();
        let mut hc = Uhci::new();
        hc.write(8, &0x1000u32.to_le_bytes());
        hc.write(16, &0x0095u16.to_le_bytes());
        hc.write(0, &1u16.to_le_bytes());
        hc
    }

    fn descriptor_request(mem: &GuestMemoryMmap, length: u32, flags: u32) -> Uhci {
        let hc = schedule(mem);
        mem.write_slice(&[0x80, 6, 0, 1, 0, 0, 18, 0], GuestAddress(0x4000))
            .unwrap();
        put_td(
            mem,
            0x3000,
            [0x3020, ACTIVE | IOC, (7 << 21) | 0x2d, 0x4000],
        );
        put_td(
            mem,
            0x3020,
            [
                1,
                ACTIVE | flags,
                ((length - 1) << 21) | (1 << 19) | 0x69,
                0x5000,
            ],
        );
        hc
    }

    #[test]
    fn registers_reset_port_and_stopped_frame_writes() {
        let mut hc = Uhci::new();
        hc.write(6, &[7]);
        assert_eq!(hc.registers.frame, 0);
        hc.write(6, &2047u16.to_le_bytes());
        hc.write(16, &0x0097u16.to_le_bytes());
        assert_eq!(hc.registers.ports[0], 0x95);
        Uhci::restore(&hc.save()).unwrap();
        hc.write(0, &1u16.to_le_bytes());
        hc.write(6, &42u16.to_le_bytes());
        assert_eq!(hc.registers.frame, 2047);
        hc.write(16, &0x0295u16.to_le_bytes());
        assert_eq!(hc.registers.ports[0] & 4, 0);
        hc.write(0, &2u16.to_le_bytes());
        assert_eq!(hc.registers.frame, 0);
        assert_eq!(hc.registers.command, 0);
        assert_eq!(hc.registers.status, HALTED);
    }

    #[test]
    fn descriptor_dma_and_control_phase_survive_restore() {
        let mem = memory();
        let mut hc = descriptor_request(&mem, 8, IOC);
        hc.write(4, &4u16.to_le_bytes());
        hc.tick(&mem);
        assert_eq!(read_word(&mem, 0x2004).unwrap(), 0x3020);
        assert!(hc.interrupt_pending());
        hc.write(2, &1u16.to_le_bytes());
        assert!(!hc.interrupt_pending());
        let mut hc = Uhci::restore(&hc.save()).unwrap();
        let region = mem.find_region(GuestAddress(0)).unwrap();
        mem.reset_dirty();
        hc.tick(&mem);
        let mut data = [0; 8];
        mem.read_slice(&mut data, GuestAddress(0x5000)).unwrap();
        assert_eq!(&data[..2], &[18, 1]);
        assert_eq!(data[7], 8);
        assert_eq!(read_word(&mem, 0x2004).unwrap(), 1);
        assert_eq!(read_word(&mem, 0x3024).unwrap() & (ACTIVE | 2047), 7);
        assert!(region.bitmap().dirty_at(0x5000));
        assert!(region.bitmap().dirty_at(0x3000));
        assert!(region.bitmap().dirty_at(0x2000));
        assert!(hc.interrupt_pending());
    }

    #[test]
    fn short_packet_stops_queue_and_has_separate_interrupt_enable() {
        let mem = memory();
        let mut hc = descriptor_request(&mem, 64, SHORT);
        hc.tick(&mem);
        hc.write(2, &1u16.to_le_bytes());
        hc.write(4, &4u16.to_le_bytes());
        hc.tick(&mem);
        assert_eq!(read_word(&mem, 0x2004).unwrap(), 0x3020);
        assert_eq!(read_word(&mem, 0x3024).unwrap() & ACTIVE, 0);
        assert!(!hc.interrupt_pending());
        hc.write(4, &8u16.to_le_bytes());
        assert!(hc.interrupt_pending());
        hc.write(2, &1u16.to_le_bytes());
        assert!(!hc.interrupt_pending());
    }

    #[test]
    fn missing_device_times_out_without_advancing_queue() {
        let mem = memory();
        let mut hc = schedule(&mem);
        put_td(
            &mem,
            0x3000,
            [1, ACTIVE | (3 << 27), (7 << 21) | (127 << 8) | 0x69, 0x5000],
        );
        hc.write(4, &1u16.to_le_bytes());
        for remaining in [2, 1, 0] {
            hc.tick(&mem);
            let status = read_word(&mem, 0x3004).unwrap();
            assert_eq!((status >> 27) & 3, remaining);
            assert_ne!(status & TIMEOUT, 0);
            assert_eq!(status & ACTIVE != 0, remaining != 0);
        }
        assert_eq!(read_word(&mem, 0x2004).unwrap(), 0x3000);
        assert_ne!(read_word(&mem, 0x3004).unwrap() & STALL, 0);
        assert!(hc.interrupt_pending());
    }

    #[test]
    fn cyclic_empty_schedule_is_bounded_and_frame_wraps() {
        let mem = memory();
        let mut hc = schedule(&mem);
        write_word(&mem, 0x2000, 0x2002).unwrap();
        write_word(&mem, 0x2004, 1).unwrap();
        hc.registers.frame = 2047;
        hc.tick(&mem);
        assert_eq!(hc.registers.frame, 0);
        assert_eq!(hc.registers.status & (PROCESS_ERROR | HOST_ERROR), 0);
        hc.tick(&mem);
        assert_eq!(hc.registers.frame, 1);
    }

    #[test]
    fn invalid_dma_halts_without_accessing_host_memory() {
        let mem = memory();
        let mut hc = schedule(&mem);
        put_td(&mem, 0x3000, [1, ACTIVE, (7 << 21) | 0x2d, 0xffff_fffc]);
        hc.tick(&mem);
        assert_eq!(
            hc.registers.status & (HOST_ERROR | HALTED),
            HOST_ERROR | HALTED
        );
        assert_eq!(read_word(&mem, 0x3004).unwrap(), ACTIVE);
        assert!(hc.interrupt_pending());
        hc.write(2, &HOST_ERROR.to_le_bytes());
        assert!(!hc.interrupt_pending());
        hc.write(8, &0xffff_f000u32.to_le_bytes());
        hc.write(0, &1u16.to_le_bytes());
        hc.tick(&mem);
        assert!(hc.interrupt_pending());
    }

    #[test]
    fn illegal_packet_pid_is_a_process_error() {
        let mem = memory();
        let mut hc = schedule(&mem);
        put_td(&mem, 0x3000, [1, ACTIVE, 0xff, 0]);
        hc.tick(&mem);
        assert_eq!(hc.registers.command & RUN, 0);
        assert_ne!(hc.registers.status & PROCESS_ERROR, 0);
        assert!(hc.interrupt_pending());
    }

    #[test]
    fn software_debug_resumes_after_exactly_one_transaction() {
        let mem = memory();
        let mut hc = descriptor_request(&mem, 8, IOC);
        write_word(&mem, 0x3000, 0x3024).unwrap();
        hc.write(0, &(RUN | DEBUG).to_le_bytes());
        hc.tick(&mem);
        assert_eq!(hc.registers.frame, 0);
        assert_eq!(hc.registers.command & RUN, 0);
        assert_ne!(read_word(&mem, 0x3024).unwrap() & ACTIVE, 0);
        let mut hc = Uhci::restore(&hc.save()).unwrap();
        hc.write(0, &(RUN | DEBUG).to_le_bytes());
        hc.tick(&mem);
        assert_eq!(read_word(&mem, 0x3024).unwrap() & ACTIVE, 0);
    }

    #[test]
    fn snapshot_rejects_reserved_registers_and_inconsistent_interrupts() {
        let mut saved = Uhci::new().save();
        saved.registers.base = 1;
        assert_eq!(
            Uhci::restore(&saved).err().unwrap().kind(),
            io::ErrorKind::Other
        );
        saved.registers.base = 0;
        saved.registers.interrupt_causes = 1;
        assert_eq!(
            Uhci::restore(&saved).err().unwrap().kind(),
            io::ErrorKind::Other
        );
        saved.registers.status |= USBINT;
        Uhci::restore(&saved).unwrap();
        saved.registers.ports[0] |= 0x204;
        assert_eq!(
            Uhci::restore(&saved).err().unwrap().kind(),
            io::ErrorKind::Other
        );
    }
}
