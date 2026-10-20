// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::mem::size_of;
use core::panic::PanicInfo;
use core::ptr::{NonNull, addr_of_mut, null_mut};
use firecracker_qxl_driver::{engine, pdev, pointer, render};

mod display_memory;

#[cfg_attr(
    target_arch = "x86",
    link(
        name = "win32k.sys",
        kind = "raw-dylib",
        modifiers = "+verbatim",
        import_name_type = "undecorated"
    )
)]
#[cfg_attr(
    target_arch = "x86_64",
    link(name = "win32k.sys", kind = "raw-dylib", modifiers = "+verbatim")
)]
unsafe extern "system" {
    fn EngBugCheckEx(code: u32, a: usize, b: usize, c: usize, d: usize) -> !;
}

#[repr(C)]
#[derive(Debug)]
struct DriverFn {
    index: u32,
    function: *const (),
}

// SAFETY: immutable table entries point only to resident code in this DLL.
unsafe impl Sync for DriverFn {}

#[repr(C)]
#[derive(Debug)]
pub struct EnableData {
    version: u32,
    count: u32,
    functions: *const DriverFn,
}

const HOOKS: u32 = 1 | 2 | 8 | 0x20 | 0x400 | 0x2000 | 0x8000 | 0x10000;

static FUNCTIONS: [DriverFn; 22] = [
    DriverFn {
        index: 0,
        function: pdev::enable_pdev as *const (),
    },
    DriverFn {
        index: 1,
        function: pdev::complete_pdev as *const (),
    },
    DriverFn {
        index: 2,
        function: pdev::disable_pdev as *const (),
    },
    DriverFn {
        index: 3,
        function: enable_surface as *const (),
    },
    DriverFn {
        index: 4,
        function: disable_surface as *const (),
    },
    DriverFn {
        index: 5,
        function: assert_mode as *const (),
    },
    DriverFn {
        index: 8,
        function: disable_driver as *const (),
    },
    DriverFn {
        index: 10,
        function: create_device_bitmap as *const (),
    },
    DriverFn {
        index: 11,
        function: pdev::delete_device_bitmap as *const (),
    },
    DriverFn {
        index: 12,
        function: firecracker_qxl_driver::brush::realize as *const (),
    },
    DriverFn {
        index: 14,
        function: render::stroke_path as *const (),
    },
    DriverFn {
        index: 18,
        function: render::bit_blt as *const (),
    },
    DriverFn {
        index: 19,
        function: render::copy_bits as *const (),
    },
    DriverFn {
        index: 20,
        function: render::stretch_blt as *const (),
    },
    DriverFn {
        index: 23,
        function: render::text_out as *const (),
    },
    DriverFn {
        index: 24,
        function: pdev::escape as *const (),
    },
    DriverFn {
        index: 29,
        function: pointer::set_pointer_shape as *const (),
    },
    DriverFn {
        index: 30,
        function: pointer::move_pointer as *const (),
    },
    DriverFn {
        index: 41,
        function: engine::get_modes as *const (),
    },
    DriverFn {
        index: 69,
        function: render::stretch_blt_rop as *const (),
    },
    DriverFn {
        index: 71,
        function: render::alpha_blend as *const (),
    },
    DriverFn {
        index: 74,
        function: render::transparent_blt as *const (),
    },
];

/// # Safety
/// GDI supplies its loader version and a writable, aligned output of size bytes.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DrvEnableDriver(
    _version: u32,
    bytes: u32,
    data: *mut EnableData,
) -> i32 {
    if bytes < size_of::<EnableData>() as u32 || data.is_null() {
        return 0;
    }
    // SAFETY: GDI serialises driver load before publishing any callbacks.
    if !unsafe { pdev::initialise_driver() } {
        return 0;
    }

    // SAFETY: validated GDI output; write fields without copying struct padding.
    unsafe {
        addr_of_mut!((*data).version).write(0x30000);
        addr_of_mut!((*data).count).write(FUNCTIONS.len() as u32);
        addr_of_mut!((*data).functions).write(FUNCTIONS.as_ptr());
    }
    1
}

unsafe extern "system" fn enable_surface(device: *mut c_void) -> *mut c_void {
    let Some(device) = NonNull::new(device) else {
        return null_mut();
    };
    // SAFETY: GDI retains the completed PDev; each advertised hook is in FUNCTIONS.
    match unsafe { pdev::enable_surface(device, HOOKS) } {
        Ok(surface) => surface.as_ptr(),
        Err(_) => null_mut(),
    }
}

unsafe extern "system" fn create_device_bitmap(
    device: *mut c_void,
    size: firecracker_qxl_driver::gdi::Size,
    format: u32,
) -> *mut c_void {
    let Some(device) = NonNull::new(device) else {
        return null_mut();
    };
    // SAFETY: GDI retains this completed PDev; every supplied hook is registered.
    unsafe { pdev::create_device_bitmap(device, size, format, HOOKS) }
        .map_or(null_mut(), NonNull::as_ptr)
}

unsafe extern "system" fn disable_surface(device: *mut c_void) {
    let Some(device) = NonNull::new(device) else {
        return;
    };
    // SAFETY: GDI ended all surface users and mode assertion stopped hardware.
    if unsafe { pdev::disable_surface(device) }.is_err() {
        panic!("QXL surface disabled before hardware stopped");
    }
}

unsafe extern "system" fn assert_mode(device: *mut c_void, enable: i32) -> i32 {
    let Some(device) = NonNull::new(device) else {
        return 0;
    };
    // SAFETY: GDI serialises mode transitions and stops drawing for the transition.
    match unsafe { pdev::assert_mode(device, enable != 0) } {
        Ok(()) => 1,
        Err(_) if enable == 0 => 0,
        Err(_) => panic!("QXL could not restore its enabled mode"),
    }
}

unsafe extern "system" fn disable_driver() {
    // SAFETY: GDI excludes callbacks and has destroyed all PDevs before unload.
    if !unsafe { pdev::shutdown_driver() } {
        panic!("QXL unloaded with live PDevs");
    }
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    let (file, length, line, column) = info.location().map_or((0, 0, 0, 0), |location| {
        (
            location.file().as_ptr() as usize,
            location.file().len(),
            location.line() as usize,
            location.column() as usize,
        )
    });
    // SAFETY: the kernel panic path cannot unwind through a Windows callback.
    unsafe { EngBugCheckEx(u32::from_be_bytes(*b"RPNC"), file, length, line, column) }
}
