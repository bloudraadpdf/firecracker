// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use crate::model::Point;
#[cfg(target_os = "windows")]
use core::ffi::c_void;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct DevMode {
    pub device_name: [u16; 32],
    pub spec_version: u16,
    pub driver_version: u16,
    pub size: u16,
    pub driver_extra: u16,
    pub fields: u32,
    pub position: Point,
    pub orientation: u32,
    pub fixed_output: u32,
    pub color: i16,
    pub duplex: i16,
    pub y_resolution: i16,
    pub tt_option: i16,
    pub collate: i16,
    pub form_name: [u16; 32],
    pub log_pixels: u16,
    pub bits: u32,
    pub width: u32,
    pub height: u32,
    pub flags: u32,
    pub frequency: u32,
    pub icm_method: u32,
    pub icm_intent: u32,
    pub media_type: u32,
    pub dither_type: u32,
    pub reserved1: u32,
    pub reserved2: u32,
    pub panning_width: u32,
    pub panning_height: u32,
}
impl DevMode {
    pub const fn empty() -> Self {
        // SAFETY: Every DEVMODE field is an integer or integer array; all-zero bits are valid.
        let mut value: Self = unsafe { core::mem::zeroed() };
        value.size = 220;
        value
    }
    pub fn valid(self) -> bool {
        let size = usize::from(self.size);
        let display_end = core::mem::offset_of!(Self, frequency) + core::mem::size_of::<u32>();
        (display_end..=core::mem::size_of::<Self>()).contains(&size) && self.driver_extra == 0
    }
    pub fn matches(self, mode: crate::model::Mode) -> bool {
        self.valid()
            && self.width == mode.width
            && self.height == mode.height
            && self.bits == 32
            && self.fields & 0x001c0000 == 0x001c0000
    }
}
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct DisplayDevice {
    pub size: u32,
    pub name: [u16; 32],
    pub description: [u16; 128],
    pub flags: u32,
    pub id: [u16; 128],
    pub key: [u16; 128],
}
impl DisplayDevice {
    pub fn empty() -> Self {
        // SAFETY: DISPLAY_DEVICE contains only integer fields and arrays, which permit zero bits.
        let mut v: Self = unsafe { core::mem::zeroed() };
        v.size = 840;
        v
    }
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Message {
    pub window: usize,
    pub message: u32,
    pub wparam: usize,
    pub lparam: isize,
    pub time: u32,
    pub point: Point,
}
#[repr(C)]
#[derive(Debug, Default)]
pub struct PaintStruct {
    pub dc: usize,
    pub erase: i32,
    pub rectangle: Rect,
    pub restore: i32,
    pub incremental: i32,
    pub reserved: [u8; 32],
}
#[repr(C)]
#[derive(Debug)]
pub struct CursorInfo {
    pub size: u32,
    pub flags: u32,
    pub cursor: usize,
    pub point: Point,
}
impl CursorInfo {
    pub fn empty() -> Self {
        Self {
            size: if cfg!(target_pointer_width = "32") {
                20
            } else {
                24
            },
            flags: 0,
            cursor: 0,
            point: Point::default(),
        }
    }
}
#[repr(C)]
#[derive(Debug)]
pub struct WindowClass {
    pub style: u32,
    pub procedure: unsafe extern "system" fn(usize, u32, usize, isize) -> isize,
    pub class_extra: i32,
    pub window_extra: i32,
    pub instance: usize,
    pub icon: usize,
    pub cursor: usize,
    pub background: usize,
    pub menu: *const u16,
    pub name: *const u16,
}
#[repr(C)]
#[derive(Debug)]
pub struct BitmapHeader {
    pub size: u32,
    pub width: i32,
    pub height: i32,
    pub planes: u16,
    pub bits: u16,
    pub compression: u32,
    pub image_size: u32,
    pub x_pixels: i32,
    pub y_pixels: i32,
    pub colors: u32,
    pub important: u32,
}

#[cfg(target_os = "windows")]
#[link(name = "kernel32")]
unsafe extern "system" {
    pub fn GetModuleHandleW(name: *const u16) -> usize;
    pub fn GetCurrentThreadId() -> u32;
    pub fn GetTickCount() -> u32;
    pub fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *const c_void,
        creation: u32,
        attributes: u32,
        template: usize,
    ) -> usize;
    pub fn FlushFileBuffers(file: usize) -> i32;
    pub fn CloseHandle(file: usize) -> i32;
}
#[cfg(target_os = "windows")]
#[link(name = "user32")]
unsafe extern "system" {
    pub fn EnumDisplayDevicesW(
        device: *const u16,
        index: u32,
        output: *mut DisplayDevice,
        flags: u32,
    ) -> i32;
    pub fn EnumDisplaySettingsW(device: *const u16, index: u32, output: *mut DevMode) -> i32;
    pub fn ChangeDisplaySettingsExW(
        device: *const u16,
        mode: *mut DevMode,
        window: usize,
        flags: u32,
        parameter: *const c_void,
    ) -> i32;
    pub fn RegisterClassW(class: *const WindowClass) -> u16;
    pub fn CreateWindowExW(
        extended: u32,
        class: *const u16,
        title: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: usize,
        menu: usize,
        instance: usize,
        parameter: *const c_void,
    ) -> usize;
    pub fn LoadCursorW(instance: usize, name: *const u16) -> usize;
    pub fn ShowWindow(window: usize, command: i32) -> i32;
    pub fn UpdateWindow(window: usize) -> i32;
    pub fn InvalidateRect(window: usize, rectangle: *const Rect, erase: i32) -> i32;
    pub fn SetWindowPos(
        window: usize,
        after: usize,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        flags: u32,
    ) -> i32;
    pub fn SetFocus(window: usize) -> usize;
    pub fn DestroyWindow(window: usize) -> i32;
    pub fn GetMessageW(message: *mut Message, window: usize, min: u32, max: u32) -> i32;
    pub fn TranslateMessage(message: *const Message) -> i32;
    pub fn DispatchMessageW(message: *const Message) -> isize;
    pub fn DefWindowProcW(window: usize, message: u32, wparam: usize, lparam: isize) -> isize;
    pub fn PostQuitMessage(code: i32);
    pub fn BeginPaint(window: usize, paint: *mut PaintStruct) -> usize;
    pub fn EndPaint(window: usize, paint: *const PaintStruct) -> i32;
    pub fn GetClientRect(window: usize, rectangle: *mut Rect) -> i32;
    pub fn GetCursorPos(point: *mut Point) -> i32;
    pub fn GetKeyState(key: i32) -> i16;
    pub fn SetTimer(
        window: usize,
        id: usize,
        interval: u32,
        callback: Option<unsafe extern "system" fn(usize, u32, usize, u32)>,
    ) -> usize;
    pub fn KillTimer(window: usize, id: usize) -> i32;
    pub fn ShowCursor(show: i32) -> i32;
    pub fn GetCursorInfo(info: *mut CursorInfo) -> i32;
}
#[cfg(target_os = "windows")]
#[link(name = "gdi32")]
unsafe extern "system" {
    pub fn GetDeviceCaps(dc: usize, index: i32) -> i32;
    pub fn SetDIBitsToDevice(
        dc: usize,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        source_x: i32,
        source_y: i32,
        start: u32,
        lines: u32,
        bits: *const c_void,
        info: *const BitmapHeader,
        usage: u32,
    ) -> i32;
    pub fn GdiFlush() -> i32;
}
const _: () = {
    assert!(core::mem::size_of::<Point>() == 8);
    assert!(core::mem::size_of::<DevMode>() == 220);
    assert!(core::mem::offset_of!(DevMode, bits) == 168);
    assert!(core::mem::offset_of!(DevMode, width) == 172);
    assert!(core::mem::offset_of!(DevMode, frequency) == 184);
    assert!(core::mem::size_of::<DisplayDevice>() == 840);
    assert!(core::mem::size_of::<Rect>() == 16);
    assert!(core::mem::size_of::<BitmapHeader>() == 40);
};
#[cfg(target_pointer_width = "32")]
const _: () = {
    assert!(core::mem::size_of::<Message>() == 28);
    assert!(core::mem::offset_of!(Message, lparam) == 12);
    assert!(core::mem::size_of::<PaintStruct>() == 64);
    assert!(core::mem::offset_of!(PaintStruct, rectangle) == 8);
    assert!(core::mem::size_of::<CursorInfo>() == 20);
    assert!(core::mem::offset_of!(CursorInfo, point) == 12);
    assert!(core::mem::size_of::<WindowClass>() == 40);
    assert!(core::mem::offset_of!(WindowClass, instance) == 16);
    assert!(core::mem::offset_of!(WindowClass, name) == 36);
};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn enumerated_mode_requires_public_layout_and_all_three_fields() {
        assert_eq!(
            usize::try_from(CursorInfo::empty().size).unwrap(),
            core::mem::size_of::<CursorInfo>()
        );
        assert_eq!(
            usize::try_from(DisplayDevice::empty().size).unwrap(),
            core::mem::size_of::<DisplayDevice>()
        );
        let expected = crate::model::Mode::at(12).unwrap();
        let mut mode = DevMode::empty();
        mode.width = expected.width;
        mode.height = expected.height;
        mode.bits = 32;
        mode.fields = 0x001c0000;
        assert!(mode.matches(expected));
        for size in [188, 220] {
            mode.size = size;
            mode.fields = 0x001c0000;
            mode.bits = 32;
            assert!(mode.matches(expected));
            mode.driver_extra = 1;
            assert!(!mode.matches(expected));
            mode.driver_extra = 0;
            for bit in [0x00040000, 0x00080000, 0x00100000] {
                mode.fields = 0x001c0000 ^ bit;
                assert!(!mode.matches(expected));
            }
            mode.fields = 0x001c0000;
            mode.width = expected.width + 1;
            assert!(!mode.matches(expected));
            mode.width = expected.width;
            mode.height = expected.height + 1;
            assert!(!mode.matches(expected));
            mode.height = expected.height;
            mode.bits = 16;
            assert!(!mode.matches(expected));
        }
    }
    #[test]
    fn public_display_extent_stays_inside_supplied_storage() {
        let mut mode = DevMode::empty();
        assert_eq!(mode.size, 220);
        for size in [0, 187, 221, u16::MAX] {
            mode.size = size;
            assert!(!mode.valid());
        }
        for size in [188, 189, 212, 219, 220] {
            mode.size = size;
            assert!(mode.valid());
            mode.driver_extra = 1;
            assert!(!mode.valid());
            mode.driver_extra = 0;
        }
    }
}
