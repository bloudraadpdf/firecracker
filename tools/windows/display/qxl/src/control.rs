// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Adapter control commands from display/qxldd.h and display/driver.c.

use core::sync::atomic::{Ordering, fence};

use crate::driver_info::DriverInfo;
use crate::memory::{REVISION_ASYNC, REVISION_MONITORS};
use crate::surface::Layout;

const STATUS_SUCCESS: i32 = 0;
const STATUS_TIMEOUT: i32 = 0x102;
const TIMEOUT_100NS: i64 = -60 * 10_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Failed,
    Unsupported,
    InvalidRequest,
    InvalidPort,
    Timeout,
    Wait(i32),
}

#[derive(Clone, Copy, Debug)]
pub enum Operation {
    Update { surface: u32, area: [i32; 4] },
    AddSlot { slot: u8, start: u64, end: u64 },
    CreatePrimary { layout: Layout, physical: u64 },
    DestroyPrimary,
    DestroySurface(u32),
    DestroyAll,
    FlushSurfaces,
    Monitors(u64),
    DeleteSlot(u8),
    FlushRelease,
}

impl Operation {
    fn port(self, info: &DriverInfo) -> Result<(u16, bool), Error> {
        let asynchronous = info.pci_revision >= REVISION_ASYNC;
        let port = match self {
            Self::Update { .. } => {
                if asynchronous {
                    info.update_area_async_port
                } else {
                    info.update_area_port
                }
            }
            Self::AddSlot { .. } => {
                if asynchronous {
                    info.memslot_add_async_port
                } else {
                    info.memslot_add_port
                }
            }
            Self::CreatePrimary { .. } => {
                if asynchronous {
                    info.create_primary_async_port
                } else {
                    info.create_primary_port
                }
            }
            Self::DestroyPrimary => {
                if asynchronous {
                    info.destroy_primary_async_port
                } else {
                    info.destroy_primary_port
                }
            }
            Self::DestroySurface(_) => {
                if asynchronous {
                    info.destroy_surface_async_port
                } else {
                    info.destroy_surface_wait_port
                }
            }
            Self::DestroyAll => {
                if asynchronous {
                    info.destroy_all_surfaces_async_port
                } else {
                    info.destroy_all_surfaces_port
                }
            }
            Self::FlushSurfaces if asynchronous => info.flush_surfaces_async_port,
            Self::Monitors(_) if info.pci_revision >= REVISION_MONITORS => {
                info.monitors_config_port
            }
            Self::DeleteSlot(_) => info.memslot_delete_port,
            Self::FlushRelease if asynchronous => info.flush_release_port,
            _ => return Err(Error::Unsupported),
        };
        let asynchronous =
            asynchronous && !matches!(self, Self::DeleteSlot(_) | Self::FlushRelease);
        if port.is_null() || port.addr() > u16::MAX as usize {
            return Err(Error::InvalidPort);
        }
        Ok((port.addr() as u16, asynchronous))
    }

    fn validate(self, info: &DriverInfo) -> Result<(), Error> {
        let valid = match self {
            // QXLRect is top, left, bottom, right.
            Self::Update {
                surface,
                area: [top, left, bottom, right],
            } => {
                surface < info.surface_count
                    && top >= 0
                    && left >= 0
                    && bottom > top
                    && right > left
            }
            Self::DestroySurface(surface) => surface != 0 && surface < info.surface_count,
            Self::AddSlot { slot, start, end } => {
                Some(slot) == info.main_slot_id.checked_add(1)
                    && slot < info.slot_count
                    && start < end
            }
            Self::DeleteSlot(slot) => {
                Some(slot) == info.main_slot_id.checked_add(1) && slot < info.slot_count
            }
            Self::CreatePrimary { layout, physical } => {
                layout.bytes() <= info.primary_area_size as usize
                    && crate::slots::SlotLayout::new(info.slot_id_bits, info.slot_generation_bits)
                        .and_then(|layout| {
                            layout.slot(
                                info.main_slot_id,
                                info.main_slot.generation,
                                info.main_slot.physical_start,
                                info.main_slot.virtual_start,
                                info.main_slot
                                    .physical_end
                                    .saturating_sub(info.main_slot.physical_start),
                            )
                        })
                        .and_then(|slot| {
                            slot.encode(info.primary_area.addr() as u64, layout.bytes() as u64)
                        })
                        == Ok(physical)
            }
            Self::Monitors(physical) => physical != 0,
            _ => true,
        };
        if valid {
            Ok(())
        } else {
            Err(Error::InvalidRequest)
        }
    }

    fn prepare(self, info: &DriverInfo, transport: &mut impl Transport) -> u8 {
        match self {
            Self::Update { surface, area } => {
                for (index, value) in area.into_iter().enumerate() {
                    transport.write(
                        info.update_area.cast::<u8>().wrapping_add(index * 4),
                        &value.to_le_bytes(),
                    );
                }
                transport.write(info.update_surface.cast(), &surface.to_le_bytes());
            }
            Self::AddSlot { slot, start, end } => {
                transport.write(info.ram_slot_start.cast(), &start.to_le_bytes());
                transport.write(info.ram_slot_end.cast(), &end.to_le_bytes());
                return slot;
            }
            Self::DeleteSlot(slot) => return slot,
            Self::CreatePrimary { layout, physical } => transport.write(
                info.primary_surface_create.cast(),
                &layout.primary(physical),
            ),
            Self::DestroySurface(surface) => {
                transport.write(info.update_surface.cast(), &surface.to_le_bytes())
            }
            Self::Monitors(physical) => {
                transport.write(info.monitors_config.cast(), &physical.to_le_bytes())
            }
            _ => {}
        }
        0
    }
}

trait Transport {
    fn clear(&mut self, event: *mut core::ffi::c_void);
    fn write(&mut self, pointer: *mut u8, bytes: &[u8]);
    fn issue(&mut self, port: u16, value: u8);
    fn wait(&mut self, event: *mut core::ffi::c_void, timeout: i64) -> i32;
}

#[derive(Debug)]
struct Channel<'mapping> {
    info: &'mapping DriverInfo,
    failed: bool,
}

impl Channel<'_> {
    fn issue(&mut self, operation: Operation, transport: &mut impl Transport) -> Result<(), Error> {
        if self.failed {
            return Err(Error::Failed);
        }
        let (port, asynchronous) = operation.port(self.info)?;
        operation.validate(self.info)?;
        if asynchronous {
            transport.clear(self.info.io_command_event);
        }
        let value = operation.prepare(self.info, transport);
        fence(Ordering::SeqCst);
        transport.issue(port, value);
        if asynchronous {
            let status = transport.wait(self.info.io_command_event, TIMEOUT_100NS);
            if status != STATUS_SUCCESS {
                self.failed = true;
                return Err(if status == STATUS_TIMEOUT {
                    Error::Timeout
                } else {
                    Error::Wait(status)
                });
            }
        }
        fence(Ordering::Acquire);
        Ok(())
    }
}

#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
mod native {
    use super::*;
    use crate::display_device::MappedDevice;
    use crate::engine::EngClearEvent;
    use core::ffi::c_void;

    #[cfg_attr(
        target_arch = "x86",
        link(
            name = "ntoskrnl.exe",
            kind = "raw-dylib",
            modifiers = "+verbatim",
            import_name_type = "undecorated"
        )
    )]
    #[cfg_attr(
        target_arch = "x86_64",
        link(name = "ntoskrnl.exe", kind = "raw-dylib", modifiers = "+verbatim")
    )]
    unsafe extern "system" {
        fn KeWaitForSingleObject(
            object: *mut c_void,
            reason: i32,
            mode: i8,
            alertable: u8,
            timeout: *const i64,
        ) -> i32;
    }

    struct Native;

    impl Transport for Native {
        fn clear(&mut self, event: *mut c_void) {
            // SAFETY: Control's constructor retains the live miniport event;
            // exclusive adapter access excludes a previous outstanding command.
            unsafe { EngClearEvent(event) };
        }
        fn write(&mut self, pointer: *mut u8, bytes: &[u8]) {
            for (index, byte) in bytes.iter().copied().enumerate() {
                // SAFETY: the validated interface supplies the complete shared
                // record; packed fields require bytes, not aligned references.
                unsafe { pointer.add(index).write_volatile(byte) };
            }
        }
        fn issue(&mut self, port: u16, value: u8) {
            // SAFETY: validated ports belong to the mapped QXL adapter. This
            // kernel callback has I/O privilege and exclusive adapter control.
            unsafe {
                core::arch::asm!("out dx, al", in("dx") port, in("al") value,
                options(nostack, preserves_flags))
            };
        }
        fn wait(&mut self, event: *mut c_void, timeout: i64) -> i32 {
            // SAFETY: the miniport's nonpaged synchronization event remains
            // live. This passive-level, non-alertable KernelMode wait keeps its
            // timeout on the kernel stack; Executive=0 and KernelMode=0.
            unsafe { KeWaitForSingleObject(event, 0, 0, 0, &timeout) }
        }
    }

    #[derive(Debug)]
    pub struct Control<'mapping>(Channel<'mapping>);

    impl<'mapping> Control<'mapping> {
        /// # Safety
        /// Create exactly one channel for the physical adapter, shared across
        /// all its PDevs under exclusive adapter access at PASSIVE_LEVEL. The
        /// hardware must have no outstanding control command. Retain the mapped
        /// device, miniport events and mappings until all hardware access stops.
        /// A replacement channel requires explicit quiescence/reset; dropping a
        /// failed channel or its timeout does not provide that guarantee.
        pub unsafe fn new(device: &'mapping MappedDevice) -> Self {
            Self(Channel {
                info: device.info(),
                failed: false,
            })
        }

        /// Issue a control command and wait for its completion.
        /// # Safety
        /// Keep control access at PASSIVE_LEVEL and exclude all other adapter
        /// control paths, including mode changes and power transitions. Surface
        /// IDs, rectangles and slot ranges must describe live adapter resources.
        /// AddSlot must use this adapter's video BAR; Monitors must reference a
        /// live valid QXLMonitorsConfig. Keep all referenced memory live through
        /// host use, including after any error, until hardware quiescence. Only
        /// successful destroy completion permits releasing affected resources.
        pub unsafe fn issue(&mut self, operation: Operation) -> Result<(), Error> {
            self.0.issue(operation, &mut Native)
        }

        pub fn failed(&self) -> bool {
            self.0.failed
        }
    }
}

#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
pub use native::Control;

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    #[derive(Debug, PartialEq, Eq)]
    enum Action {
        Clear(usize),
        Write(usize, Vec<u8>),
        Issue(u16, u8),
        Wait(usize, i64),
    }

    #[derive(Default)]
    struct Fake {
        actions: Vec<Action>,
        status: i32,
    }

    impl Transport for Fake {
        fn clear(&mut self, event: *mut core::ffi::c_void) {
            self.actions.push(Action::Clear(event.addr()));
        }
        fn write(&mut self, pointer: *mut u8, bytes: &[u8]) {
            self.actions
                .push(Action::Write(pointer.addr(), bytes.into()));
        }
        fn issue(&mut self, port: u16, value: u8) {
            self.actions.push(Action::Issue(port, value));
        }
        fn wait(&mut self, event: *mut core::ffi::c_void, timeout: i64) -> i32 {
            self.actions.push(Action::Wait(event.addr(), timeout));
            self.status
        }
    }

    fn pointer<T>(address: usize) -> *mut T {
        core::ptr::without_provenance_mut(address)
    }

    fn info(revision: u8) -> DriverInfo {
        DriverInfo {
            pci_revision: revision,
            surface_count: 256,
            slot_count: 3,
            main_slot_id: 1,
            slot_id_bits: 8,
            slot_generation_bits: 8,
            main_slot: crate::driver_info::MemSlot {
                generation: 4,
                physical_start: 0xe0000000,
                physical_end: 0xe0003000,
                virtual_start: 0x10000000,
                virtual_end: 0x10003000,
            },
            primary_area: pointer(0x10000000),
            primary_area_size: 4096,
            update_area: pointer(0x10005488),
            update_surface: pointer(0x10005498),
            ram_slot_start: pointer(0x1000549c),
            ram_slot_end: pointer(0x100054a4),
            primary_surface_create: pointer(0x100054ac),
            monitors_config: pointer(0x100054dc),
            io_command_event: pointer(0x30000000),
            update_area_port: pointer(0xc002),
            update_area_async_port: pointer(0xc010),
            memslot_add_port: pointer(0xc008),
            memslot_add_async_port: pointer(0xc011),
            memslot_delete_port: pointer(0xc009),
            create_primary_port: pointer(0xc00c),
            create_primary_async_port: pointer(0xc012),
            destroy_primary_port: pointer(0xc00d),
            destroy_primary_async_port: pointer(0xc013),
            destroy_surface_wait_port: pointer(0xc00e),
            destroy_surface_async_port: pointer(0xc014),
            destroy_all_surfaces_port: pointer(0xc00f),
            destroy_all_surfaces_async_port: pointer(0xc015),
            flush_surfaces_async_port: pointer(0xc016),
            flush_release_port: pointer(0xc017),
            monitors_config_port: pointer(0xc018),
            ..DriverInfo::default()
        }
    }

    #[test]
    fn clear_payload_issue_wait_are_one_ordered_operation() {
        let info = info(5);
        let mut channel = Channel {
            info: &info,
            failed: false,
        };
        let mut fake = Fake::default();
        channel
            .issue(
                Operation::AddSlot {
                    slot: 2,
                    start: 0xf0000000,
                    end: 0xf0010000,
                },
                &mut fake,
            )
            .unwrap();
        assert_eq!(
            fake.actions,
            [
                Action::Clear(0x30000000),
                Action::Write(0x1000549c, 0xf0000000u64.to_le_bytes().into()),
                Action::Write(0x100054a4, 0xf0010000u64.to_le_bytes().into()),
                Action::Issue(0xc011, 2),
                Action::Wait(0x30000000, -600000000)
            ]
        );
        assert!(!channel.failed);
    }

    #[test]
    fn timeout_and_other_nonzero_statuses_latch_failure_without_fallback() {
        for status in [STATUS_TIMEOUT, 0xc0000001u32 as i32, 0xc0, 0x101] {
            let info = info(5);
            let mut channel = Channel {
                info: &info,
                failed: false,
            };
            let mut fake = Fake {
                status,
                ..Fake::default()
            };
            assert_eq!(
                channel.issue(Operation::DestroyAll, &mut fake),
                Err(if status == STATUS_TIMEOUT {
                    Error::Timeout
                } else {
                    Error::Wait(status)
                })
            );
            let count = fake.actions.len();
            assert_eq!(
                channel.issue(Operation::DestroyPrimary, &mut fake),
                Err(Error::Failed)
            );
            assert_eq!(
                channel.issue(Operation::DeleteSlot(2), &mut fake),
                Err(Error::Failed)
            );
            assert_eq!(fake.actions.len(), count);
        }
    }

    #[test]
    fn legacy_and_revision_gated_commands_use_the_correct_ports() {
        let info = info(2);
        let mut channel = Channel {
            info: &info,
            failed: false,
        };
        let mut fake = Fake::default();
        channel.issue(Operation::DestroyPrimary, &mut fake).unwrap();
        assert_eq!(fake.actions, [Action::Issue(0xc00d, 0)]);
        for operation in [
            Operation::FlushSurfaces,
            Operation::FlushRelease,
            Operation::Monitors(1),
        ] {
            assert_eq!(channel.issue(operation, &mut fake), Err(Error::Unsupported));
        }
        assert_eq!(fake.actions.len(), 1);
        assert!(!channel.failed);
        let info = self::info(3);
        let mut channel = Channel {
            info: &info,
            failed: false,
        };
        assert_eq!(
            channel.issue(Operation::Monitors(1), &mut fake),
            Err(Error::Unsupported)
        );
        channel.issue(Operation::FlushRelease, &mut fake).unwrap();
        assert_eq!(fake.actions.last(), Some(&Action::Issue(0xc017, 0)));
    }

    #[test]
    fn invalid_operations_have_no_memory_or_port_side_effects() {
        let mut info = info(5);
        info.destroy_primary_async_port = pointer(0x10000);
        let mut channel = Channel {
            info: &info,
            failed: false,
        };
        let mut fake = Fake::default();
        for operation in [
            Operation::DestroySurface(0),
            Operation::DestroySurface(256),
            Operation::AddSlot {
                slot: 1,
                start: 0,
                end: 4096,
            },
            Operation::AddSlot {
                slot: 2,
                start: 4096,
                end: 0,
            },
            Operation::DeleteSlot(1),
            Operation::Update {
                surface: 0,
                area: [0, 0, 0, 1],
            },
            Operation::Update {
                surface: 256,
                area: [0, 0, 1, 1],
            },
            Operation::Monitors(0),
            Operation::CreatePrimary {
                layout: Layout::new(8, 8, crate::surface::Format::Xrgb8888).unwrap(),
                physical: 1,
            },
        ] {
            assert_eq!(
                channel.issue(operation, &mut fake),
                Err(Error::InvalidRequest)
            );
        }
        assert_eq!(
            channel.issue(Operation::DestroyPrimary, &mut fake),
            Err(Error::InvalidPort)
        );
        assert!(fake.actions.is_empty());
        assert!(!channel.failed);
    }

    #[test]
    fn primary_update_and_monitor_payloads_preserve_packed_wire_order() {
        let info = info(5);
        let mut channel = Channel {
            info: &info,
            failed: false,
        };
        let mut fake = Fake::default();
        let layout = Layout::new(8, 8, crate::surface::Format::Xrgb8888).unwrap();
        channel
            .issue(
                Operation::CreatePrimary {
                    layout,
                    physical: 0x0104000000000000,
                },
                &mut fake,
            )
            .unwrap();
        assert_eq!(
            fake.actions[1],
            Action::Write(0x100054ac, layout.primary(0x0104000000000000).into())
        );
        fake.actions.clear();
        channel
            .issue(
                Operation::Update {
                    surface: 7,
                    area: [1, 2, 3, 4],
                },
                &mut fake,
            )
            .unwrap();
        for (index, value) in [1i32, 2, 3, 4].into_iter().enumerate() {
            assert_eq!(
                fake.actions[index + 1],
                Action::Write(0x10005488 + index * 4, value.to_le_bytes().into())
            );
        }
        assert_eq!(
            fake.actions[5],
            Action::Write(0x10005498, 7u32.to_le_bytes().into())
        );
        fake.actions.clear();
        channel
            .issue(Operation::Monitors(0x0104000000001000), &mut fake)
            .unwrap();
        assert_eq!(
            fake.actions[1],
            Action::Write(0x100054dc, 0x0104000000001000u64.to_le_bytes().into())
        );
    }

    #[test]
    fn protocol_port_table_covers_every_control_operation() {
        let layout = Layout::new(8, 8, crate::surface::Format::Xrgb8888).unwrap();
        for (operation, legacy, asynchronous) in [
            (
                Operation::Update {
                    surface: 0,
                    area: [0, 0, 1, 1],
                },
                0xc002,
                0xc010,
            ),
            (
                Operation::AddSlot {
                    slot: 2,
                    start: 0,
                    end: 4096,
                },
                0xc008,
                0xc011,
            ),
            (
                Operation::CreatePrimary {
                    layout,
                    physical: 0x0104000000000000,
                },
                0xc00c,
                0xc012,
            ),
            (Operation::DestroyPrimary, 0xc00d, 0xc013),
            (Operation::DestroySurface(7), 0xc00e, 0xc014),
            (Operation::DestroyAll, 0xc00f, 0xc015),
        ] {
            assert_eq!(operation.port(&info(2)), Ok((legacy, false)));
            assert_eq!(operation.port(&info(3)), Ok((asynchronous, true)));
        }
        assert_eq!(Operation::FlushSurfaces.port(&info(3)), Ok((0xc016, true)));
        assert_eq!(Operation::Monitors(1).port(&info(4)), Ok((0xc018, true)));
        assert_eq!(Operation::DeleteSlot(2).port(&info(2)), Ok((0xc009, false)));
        assert_eq!(Operation::DeleteSlot(2).port(&info(3)), Ok((0xc009, false)));
        assert_eq!(Operation::FlushRelease.port(&info(3)), Ok((0xc017, false)));
    }
}
