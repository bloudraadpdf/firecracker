// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! VideoPort callback registration for the QXL miniport.

use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::mem::{MaybeUninit, size_of};
use core::ptr::{NonNull, null_mut, read_unaligned, write, write_unaligned};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::adapter::Adapter;
use crate::driver_info::GET_INFO;
use crate::resources::DeviceResources;
use crate::videoport::*;

struct Extension {
    ready: AtomicBool,
    adapter: UnsafeCell<MaybeUninit<Adapter>>,
}

#[repr(C)]
#[derive(Default)]
struct Version {
    size: u32,
    major: u32,
    minor: u32,
    build: u32,
    service_pack_major: u16,
    service_pack_minor: u16,
}

const _: () = assert!(size_of::<Version>() == 20);

/// # Safety
/// Invoke only as the miniport's DriverEntry, passing the OS-provided arguments
/// at PASSIVE_LEVEL. The registered callbacks and code must remain resident.
pub unsafe extern "system" fn driver_entry(argument1: *mut c_void, argument2: *mut c_void) -> u32 {
    // SAFETY: this ABI record contains only integers, raw pointers and nullable
    // function pointers, all of which admit the all-zero representation.
    let mut init: InitializationData = unsafe { core::mem::zeroed() };
    init.size = size_of::<InitializationData>() as u32;
    init.device_extension_size = size_of::<Extension>() as u32;
    init.find_adapter = Some(find_adapter);
    init.initialize = Some(initialize);
    init.interrupt = Some(interrupt);
    init.start_io = Some(start_io);
    init.get_power_state = Some(get_power);
    init.set_power_state = Some(set_power);
    init.get_child_descriptor = Some(child_descriptor);
    // SAFETY: the registration record remains live for this synchronous call;
    // its callbacks match the documented system ABI and extension allocation.
    unsafe { VideoPortInitialize(argument1, argument2, &mut init, null_mut()) }
}

unsafe fn adapter<'a>(device: *mut c_void) -> Option<&'a Adapter> {
    // SAFETY: callbacks receive the extension allocated by VideoPortInitialize.
    // FindAdapter initialises it before the framework enables other callbacks.
    let extension = unsafe { device.cast::<Extension>().as_ref() }?;
    if !extension.ready.load(Ordering::Acquire) {
        return None;
    }
    // SAFETY: release publication occurs only after the Adapter is initialised.
    // Callback borrows cannot outlive the registered extension.
    Some(unsafe { (&*extension.adapter.get()).assume_init_ref() })
}

unsafe extern "system" fn find_adapter(
    device: *mut c_void,
    _: *mut c_void,
    _: *mut u16,
    config: *mut ConfigInfoPrefix,
    again: *mut u8,
) -> u32 {
    let Some(device) = NonNull::new(device) else {
        return 87;
    };
    let extension = device.as_ptr().cast::<Extension>();
    // SAFETY: this is fresh, zeroed extension storage, exclusively owned during
    // FindAdapter. No callback can inspect Adapter before release publication.
    unsafe {
        write(
            extension,
            Extension {
                ready: AtomicBool::new(false),
                adapter: UnsafeCell::new(MaybeUninit::uninit()),
            },
        )
    };
    if !again.is_null() {
        // SAFETY: the framework supplies this writable single-byte output.
        unsafe { write(again, 0) };
    }
    let mut version = Version {
        size: size_of::<Version>() as u32,
        ..Version::default()
    };
    // SAFETY: version is a writable complete VPOSVERSIONINFO record.
    let status =
        unsafe { VideoPortGetVersion(device.as_ptr(), (&mut version as *mut Version).cast()) };
    if status != 0 || version.major < 5 || version.major == 5 && version.minor < 1 {
        return 50;
    }
    // SAFETY: this is the owning FindAdapter call at PASSIVE_LEVEL. Errors drop
    // every acquired resource and release the uncommitted resource claim here.
    let result = unsafe {
        DeviceResources::discover(device, config).and_then(|resources| Adapter::new(resources))
    };
    let mut adapter = match result {
        Ok(adapter) => adapter,
        Err(status) => return status,
    };
    let mut name = (*b"QxlDeviceID\0").map(u16::from);
    let mut id = adapter.resources.info.device_id();
    // SAFETY: the name and value buffers stay live for this synchronous call.
    // Like the original miniport, this diagnostic value is best effort.
    unsafe {
        VideoPortSetRegistryParameters(
            device.as_ptr(),
            name.as_mut_ptr(),
            (&mut id as *mut u32).cast(),
            4,
        )
    };
    adapter.resources.commit_discovery();
    // SAFETY: all fallible setup has finished. Publish once, before returning
    // success; interrupt and request callbacks only receive shared Adapter refs.
    unsafe {
        (*extension).adapter.get().write(MaybeUninit::new(adapter));
        (*extension).ready.store(true, Ordering::Release);
    }
    0
}

unsafe extern "system" fn initialize(device: *mut c_void) -> u8 {
    // SAFETY: VideoPort supplies a live extension and serialises initialisation.
    unsafe { u8::from(adapter(device).is_some_and(|adapter| adapter.initialize().is_ok())) }
}

unsafe extern "system" fn interrupt(device: *mut c_void) -> u8 {
    // SAFETY: the framework invokes the resident ISR with a live extension.
    unsafe {
        u8::from(adapter(device).is_some_and(|adapter| adapter.interrupt(complete_interrupt)))
    }
}

unsafe extern "system" fn complete_interrupt(device: *mut c_void, _: *mut c_void) {
    // SAFETY: this queued DPC retains the registered device and live events.
    unsafe {
        if let Some(adapter) = adapter(device) {
            adapter.complete_interrupt();
        }
    }
}

unsafe extern "system" fn start_io(device: *mut c_void, request: *mut RequestPacket) -> u8 {
    // SAFETY: VideoPort supplies the complete request record and StatusBlock.
    let Some(request) = (unsafe { request.as_ref() }) else {
        return 1;
    };
    if request.status.is_null() {
        return 1;
    }
    // SAFETY: mode requests are serialised by VideoPort. Any potentially aliased
    // input buffer is consumed by dispatch before its output borrow is formed.
    let result = unsafe {
        match adapter(device) {
            Some(adapter) => adapter.dispatch(
                request,
                request.ioctl == GET_INFO && surfaces_enabled(device),
            ),
            None => Err(55),
        }
    };
    let (status, information) = match result {
        Ok(bytes) => (0, bytes),
        Err(status) => (status, 0),
    };
    // SAFETY: StatusBlock is a distinct writable framework record.
    unsafe {
        write_unaligned(
            request.status,
            StatusBlock {
                status,
                information,
            },
        )
    };
    1
}

unsafe fn power_request(id: u32, state: *mut c_void) -> Result<u32, u32> {
    if id != u32::MAX || state.is_null() {
        return Err(1164);
    }
    // SAFETY: the framework supplies VIDEO_POWER_MANAGEMENT; validate Length
    // before reading the remaining fields of its stable twelve-byte layout.
    let length = unsafe { read_unaligned(state.cast::<u32>()) };
    if length < 12 {
        return Err(87);
    }
    // SAFETY: the checked record includes the PowerState field at offset eight.
    let power = unsafe { read_unaligned(state.cast::<u8>().add(8).cast::<u32>()) };
    if (1..=6).contains(&power) {
        Ok(power)
    } else {
        Err(1164)
    }
}

unsafe extern "system" fn get_power(_: *mut c_void, id: u32, state: *mut c_void) -> u32 {
    // SAFETY: this is the framework's power query and its live input record.
    unsafe { power_request(id, state).map_or_else(|status| status, |_| 0) }
}

unsafe extern "system" fn set_power(device: *mut c_void, id: u32, state: *mut c_void) -> u32 {
    // SAFETY: VideoPort supplies the live power record and serialises transitions.
    unsafe {
        match power_request(id, state) {
            Ok(1) => match adapter(device) {
                Some(adapter) => adapter.resume().map_or_else(|status| status, |_| 0),
                None => 55,
            },
            Ok(_) => 0,
            Err(status) => status,
        }
    }
}

unsafe extern "system" fn child_descriptor(
    _: *mut c_void,
    _: *mut c_void,
    _: *mut i32,
    _: *mut u8,
    _: *mut u32,
    _: *mut u32,
) -> u32 {
    // As in the original XDDM miniport, there are no separately enumerated children.
    1248
}

unsafe extern "system" fn registry_value(
    _: *mut c_void,
    context: *mut c_void,
    _: *mut u16,
    value: *mut c_void,
    length: u32,
) -> u32 {
    if context.is_null() || value.is_null() || length < 4 {
        return 13;
    }
    // SAFETY: VideoPort supplies the value; context is our four-byte local output.
    unsafe { write_unaligned(context.cast::<u32>(), read_unaligned(value.cast::<u32>())) };
    0
}

unsafe fn surfaces_enabled(device: *mut c_void) -> bool {
    let mut name = (*b"SurfacesEnabled\0").map(u16::from);
    let mut value = 0u32;
    // SAFETY: the query is synchronous at PASSIVE_LEVEL; name, context and value
    // remain valid until the registered callback has finished.
    unsafe {
        VideoPortGetRegistryParameters(
            device,
            name.as_mut_ptr(),
            0,
            registry_value,
            (&mut value as *mut u32).cast(),
        ) == 0
            && value != 0
    }
}
