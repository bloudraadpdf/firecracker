// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

extern crate std;

use crate::cursor::{Command, Input, Shape, Trail};
use crate::display_resources::test_utils::{Bytes, HostRing, empty, pool};
use crate::display_resources::{Command as OutputCommand, Error, Pool, Region};
use crate::ring::CommandRing;
use crate::slots::{MemorySlot, SlotLayout};
use core::ptr::null_mut;
use std::vec::Vec;

#[path = "pointer.rs"]
mod callbacks;

#[derive(Debug)]
struct Resources<'memory> {
    pool: Pool<'memory>,
    cursor: CommandRing,
    slot: MemorySlot,
}

impl Resources<'_> {
    fn main_slot(&self) -> MemorySlot {
        self.slot
    }
}

#[derive(Debug)]
struct Hardware<'memory> {
    resources: Resources<'memory>,
    notifications: usize,
}

impl<'memory> Hardware<'memory> {
    fn resources(&mut self) -> Result<&mut Resources<'memory>, ()> {
        Ok(&mut self.resources)
    }

    fn reap(&mut self, budget: usize) -> Result<(), ()> {
        assert_eq!(budget, 64);
        Ok(())
    }

    fn notify(&mut self, cursor: bool) -> Result<(), ()> {
        assert!(cursor);
        self.notifications += 1;
        Ok(())
    }
}

mod pdev {
    use super::Hardware;
    use crate::{gdi::SurfaceObject, surface::Format};

    pub unsafe fn primary_operation<'memory>(
        surface: *mut SurfaceObject,
        operation: impl FnOnce(&mut Hardware<'memory>, Format) -> Option<u32>,
    ) -> Option<u32> {
        // SAFETY: the test passes its exclusively borrowed Hardware as the
        // opaque surface; it remains live until the actual callback returns.
        unsafe { surface.cast::<Hardware<'memory>>().as_mut() }
            .and_then(|hardware| operation(hardware, Format::Xrgb8888))
    }
}

mod engine {
    #[allow(non_snake_case)]
    pub unsafe fn XLATEOBJ_iXlate(_: *mut core::ffi::c_void, _: u32) -> u32 {
        panic!("transparent cursor must not read a palette")
    }
}

mod native_pixels {
    use crate::{cursor::Bitmap, cursor::Format, gdi::SurfaceObject};

    #[derive(Debug)]
    pub struct Pixels {
        pub format: Format,
    }

    impl Pixels {
        pub unsafe fn capture(_: *mut SurfaceObject, _: bool) -> Option<Self> {
            panic!("transparent cursor must not capture bitmap pixels")
        }

        pub fn bitmap(&self) -> Option<Bitmap<'_>> {
            panic!("transparent cursor must not reference bitmap pixels")
        }
    }
}

fn check(produced: u32, exhaust_memory: bool, accepted: bool) {
    for flags in [0, 1] {
        let mut memory = [0u64; 1024];
        let slot = SlotLayout::new(8, 8)
            .unwrap()
            .slot(
                1,
                4,
                0xe0000000,
                memory.as_ptr().addr() as u64,
                size_of_val(&memory) as u64,
            )
            .unwrap();
        let mut pool = pool(&mut memory, None);
        let mut held = Vec::new();
        if exhaust_memory {
            let shape = (flags == 1)
                .then(|| Shape::allocate(&mut pool, slot, Input::Transparent, 0).unwrap());
            let output = pool.output(OutputCommand::Cursor, 150).unwrap();
            while let Ok(resource) = pool.resource(Region::Main, 8) {
                held.push(resource);
            }
            pool.cancel(output).unwrap();
            if let Some(shape) = shape {
                shape.release(&mut pool).unwrap();
            }
        }
        let mut command_bytes = Bytes([0; 532]);
        let host = HostRing::new(&mut command_bytes, 32, 16);
        host.set(4, produced);
        host.set(8, produced + 1);
        let mut hardware = Hardware {
            resources: Resources {
                pool,
                // SAFETY: the aligned test ring remains live and this
                // callback owns its sole producer until simulated quiescence.
                cursor: unsafe { CommandRing::new(host.base, 532).unwrap() },
                slot,
            },
            notifications: 0,
        };
        let surface = core::ptr::from_mut(&mut hardware).cast();
        // SAFETY: the mock endpoint retains the exclusive Hardware pointer;
        // NULL bitmaps select the actual callback's transparent shape path.
        let result = unsafe {
            callbacks::set_pointer_shape(
                surface,
                null_mut(),
                null_mut(),
                null_mut(),
                0,
                0,
                7,
                8,
                null_mut(),
                flags,
            )
        };
        assert_eq!(
            (result, hardware.notifications),
            (if accepted { 2 } else { 0 }, usize::from(accepted)),
            "flags={flags} produced={produced} OOM={exhaust_memory}",
        );
        let publications = if !accepted {
            0
        } else if produced == 31 || exhaust_memory {
            1
        } else {
            2
        };
        assert_eq!(host.get(4), produced + publications);
        if accepted {
            let output = host.command(&hardware.resources.pool, produced);
            // SAFETY: the pending command retains its complete payload.
            let bytes = unsafe { output.snapshot::<150>() };
            assert_eq!(bytes[8], if flags == 1 { 0 } else { 1 });
            assert_eq!(&bytes[9..13], &[7, 0, 8, 0]);
        }
        if exhaust_memory {
            // SAFETY: the same live pool and ring remain exclusively owned.
            let error = unsafe {
                Command::Trail(Trail::from_flags(flags))
                    .submit(&mut hardware.resources.pool, &mut hardware.resources.cursor)
            };
            assert_eq!(error, Err(Error::OutOfMemory));
        }
        if !accepted {
            // SAFETY: the same opaque endpoint is live; a full ring must
            // also reject movement without a notification or leaked output.
            unsafe { callbacks::move_pointer(surface, 9, 10, null_mut()) };
            assert_eq!(hardware.notifications, 0);
        }
        // SAFETY: the simulated host stops reading all pending commands.
        unsafe { hardware.resources.pool.discard_pending() };
        for resource in held {
            hardware.resources.pool.release(resource).unwrap();
        }
        empty(&mut hardware.resources.pool);
    }
}

#[test]
fn full_ring_trail_failure_preserves_accepted_cursor_and_notification() {
    check(31, false, true);
}

#[test]
fn allocation_failure_in_trail_preserves_accepted_cursor_and_notification() {
    check(0, true, true);
}

#[test]
fn accepted_cursor_and_trail_share_one_notification() {
    check(0, false, true);
}

#[test]
fn rejected_primary_cursor_never_notifies_or_leaks() {
    check(32, false, false);
}
