// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

#![no_std]
#![no_main]

pub mod abi;
pub mod model;
#[path = "../../egress/win32.rs"]
pub mod win32;

use core::ptr::{null, null_mut};

use abi::*;

const WIDTH: i32 = 320;
const HEIGHT: i32 = 384;
const BYTES: u32 = (WIDTH * HEIGHT * 4) as u32;
type Result = core::result::Result<(), &'static str>;

#[repr(C)]
struct Transform {
    xx: f32,
    xy: f32,
    yx: f32,
    yy: f32,
    dx: f32,
    dy: f32,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn Sleep(milliseconds: u32);
}
#[link(name = "user32")]
unsafe extern "system" {
    fn GetDC(window: usize) -> usize;
    fn ReleaseDC(window: usize, dc: usize) -> i32;
    fn ClientToScreen(window: usize, point: *mut model::Point) -> i32;
    fn FillRect(dc: usize, rect: *const Rect, brush: usize) -> i32;
    fn GetAsyncKeyState(key: i32) -> i16;
    fn PeekMessageW(message: *mut Message, window: usize, min: u32, max: u32, remove: u32) -> i32;
}
#[link(name = "gdi32")]
unsafe extern "system" {
    fn CreateCompatibleDC(dc: usize) -> usize;
    fn DeleteDC(dc: usize) -> i32;
    fn CreateDIBSection(
        dc: usize,
        info: *const BitmapHeader,
        usage: u32,
        bits: *mut *mut u8,
        section: usize,
        offset: u32,
    ) -> usize;
    fn CreateSolidBrush(color: u32) -> usize;
    fn CreatePen(style: i32, width: i32, color: u32) -> usize;
    fn SelectObject(dc: usize, object: usize) -> usize;
    fn DeleteObject(object: usize) -> i32;
    fn SaveDC(dc: usize) -> i32;
    fn RestoreDC(dc: usize, saved: i32) -> i32;
    fn IntersectClipRect(dc: usize, left: i32, top: i32, right: i32, bottom: i32) -> i32;
    fn SetGraphicsMode(dc: usize, mode: i32) -> i32;
    fn SetWorldTransform(dc: usize, transform: *const Transform) -> i32;
    fn SetBkMode(dc: usize, mode: i32) -> i32;
    fn SetBkColor(dc: usize, color: u32) -> u32;
    fn SetROP2(dc: usize, mode: i32) -> i32;
    fn BeginPath(dc: usize) -> i32;
    fn MoveToEx(dc: usize, x: i32, y: i32, previous: *mut model::Point) -> i32;
    fn LineTo(dc: usize, x: i32, y: i32) -> i32;
    fn CloseFigure(dc: usize) -> i32;
    fn EndPath(dc: usize) -> i32;
    fn StrokePath(dc: usize) -> i32;
}

fn check(ok: bool, api: &'static str) -> Result {
    if ok { Ok(()) } else { Err(api) }
}

fn output(bytes: &[u8]) -> Result {
    let mut written = 0;
    // SAFETY: The immutable slice and count live through the synchronous write.
    let ok = unsafe {
        win32::WriteFile(
            win32::GetStdHandle(0xffff_fff5),
            bytes.as_ptr(),
            bytes.len() as u32,
            &mut written,
            null_mut(),
        )
    };
    check(ok != 0 && written as usize == bytes.len(), "stdout")
}

fn draw(dc: usize) -> Result {
    // SAFETY: The caller retains the live DC throughout these synchronous GDI calls.
    // Each selected pen is removed by RestoreDC before its deletion.
    unsafe {
        let brush = CreateSolidBrush(0x0052_7331);
        check(brush != 0, "CreateSolidBrush")?;
        let rect = Rect {
            left: 0,
            top: 0,
            right: WIDTH,
            bottom: HEIGHT,
        };
        let filled = FillRect(dc, &rect, brush);
        let deleted = DeleteObject(brush);
        check(filled != 0 && deleted != 0, "FillRect/DeleteObject")?;
        for (row, (background, rop)) in [1, 2]
            .into_iter()
            .flat_map(|bg| [13, 7, 10, 4].map(|rop| (bg, rop)))
            .enumerate()
        {
            for style in 0..5 {
                let saved = SaveDC(dc);
                check(saved != 0, "SaveDC")?;
                let pen = CreatePen(style, 0, 0x0037_d9e5);
                let result = (|| {
                    check(pen != 0, "CreatePen")?;
                    let old = SelectObject(dc, pen);
                    check(old != 0 && old != usize::MAX, "SelectObject pen")?;
                    let x = style * 64;
                    let y = row as i32 * 48;
                    check(
                        IntersectClipRect(dc, x + 1, y + 1, x + 63, y + 47) != 0,
                        "IntersectClipRect",
                    )?;
                    check(SetBkMode(dc, background) != 0, "SetBkMode")?;
                    check(SetBkColor(dc, 0x00e9_4783) != u32::MAX, "SetBkColor")?;
                    check(SetROP2(dc, rop) != 0, "SetROP2")?;
                    check(SetGraphicsMode(dc, 2) != 0, "SetGraphicsMode")?;
                    let transform = Transform {
                        xx: 1.0 / 16.0,
                        xy: 0.0,
                        yx: 0.0,
                        yy: 1.0 / 16.0,
                        dx: 0.0,
                        dy: 0.0,
                    };
                    check(SetWorldTransform(dc, &transform) != 0, "SetWorldTransform")?;
                    check(BeginPath(dc) != 0, "BeginPath")?;
                    check(
                        MoveToEx(dc, (x - 5) * 16 + 4, (y + 6) * 16 + 12, null_mut()) != 0,
                        "MoveToEx",
                    )?;
                    check(
                        LineTo(dc, (x + 60) * 16 + 8, (y + 12) * 16 + 4) != 0,
                        "LineTo first",
                    )?;
                    check(
                        LineTo(dc, (x + 3) * 16 + 12, (y + 40) * 16 + 8) != 0,
                        "LineTo second",
                    )?;
                    check(CloseFigure(dc) != 0, "CloseFigure")?;
                    check(EndPath(dc) != 0, "EndPath")?;
                    check(StrokePath(dc) != 0, "StrokePath")
                })();
                let restored = RestoreDC(dc, saved);
                let deleted = pen == 0 || DeleteObject(pen) != 0;
                result
                    .and(check(restored != 0, "RestoreDC"))
                    .and(check(deleted, "DeleteObject pen"))?;
            }
        }
        check(GdiFlush() != 0, "GdiFlush")
    }
}

fn save(bits: *const u8) -> Result {
    let name: [u16; 16] = b"gdi-oracle.bgra\0".map(u16::from);
    // SAFETY: The fixed path is terminated; the DIB allocation contains BYTES readable bytes after GdiFlush.
    unsafe {
        let file = CreateFileW(name.as_ptr(), 0x4000_0000, 0, null(), 1, 0x80, 0);
        check(file != usize::MAX, "CreateFileW (requires new oracle file)")?;
        let mut written = 0;
        let wrote = win32::WriteFile(file, bits, BYTES, &mut written, null_mut());
        let flushed = FlushFileBuffers(file);
        let closed = CloseHandle(file);
        check(
            wrote != 0 && written == BYTES && flushed != 0 && closed != 0,
            "oracle write/flush/close",
        )
    }
}

fn run() -> Result {
    let mut window = 0;
    let mut screen = 0;
    let mut memory = 0;
    let mut bitmap = 0;
    let mut old = 0;
    let name = b"QXL drawing\0".map(u16::from);
    // SAFETY: GUI objects belong to this thread. All pointer buffers outlive each call;
    // the DIB stays selected only until cleanup, after its final flushed read.
    unsafe {
        let instance = GetModuleHandleW(null());
        let class = WindowClass {
            style: 0,
            procedure: DefWindowProcW,
            class_extra: 0,
            window_extra: 0,
            instance,
            icon: 0,
            cursor: 0,
            background: 0,
            menu: null(),
            name: name.as_ptr(),
        };
        check(RegisterClassW(&class) != 0, "RegisterClassW")?;
        let result = (|| {
            window = CreateWindowExW(
                8,
                name.as_ptr(),
                name.as_ptr(),
                0x8000_0000,
                64,
                64,
                WIDTH,
                HEIGHT,
                0,
                0,
                instance,
                null(),
            );
            check(window != 0, "CreateWindowExW")?;
            ShowWindow(window, 5);
            UpdateWindow(window);
            let mut rect = Rect::default();
            let mut point = model::Point::default();
            check(
                GetClientRect(window, &mut rect) != 0
                    && ClientToScreen(window, &mut point) != 0
                    && rect.right == WIDTH
                    && rect.bottom == HEIGHT
                    && point.x == 64
                    && point.y == 64,
                "window geometry",
            )?;
            screen = GetDC(window);
            check(screen != 0, "GetDC")?;
            memory = CreateCompatibleDC(screen);
            check(memory != 0, "CreateCompatibleDC")?;
            let info = BitmapHeader {
                size: 40,
                width: WIDTH,
                height: -HEIGHT,
                planes: 1,
                bits: 32,
                compression: 0,
                image_size: BYTES,
                x_pixels: 0,
                y_pixels: 0,
                colors: 0,
                important: 0,
            };
            let mut bits = null_mut();
            bitmap = CreateDIBSection(memory, &info, 0, &mut bits, 0, 0);
            check(bitmap != 0 && !bits.is_null(), "CreateDIBSection")?;
            old = SelectObject(memory, bitmap);
            check(old != 0 && old != usize::MAX, "SelectObject bitmap")?;
            draw(memory)?;
            save(bits)?;
            draw(screen)?;
            GetAsyncKeyState(0x70);
            output(b"GDI READY x=64 y=64 width=320 height=384; capture then F1\r\n")?;
            let start = GetTickCount();
            while GetTickCount().wrapping_sub(start) < 90000 {
                if GetAsyncKeyState(0x70) & 1 != 0 {
                    return Ok(());
                }
                if GetAsyncKeyState(0x1b) < 0 {
                    return Err("Escape");
                }
                let mut message = Message::default();
                while PeekMessageW(&mut message, 0, 0, 0, 1) != 0 {
                    if message.message == 0x12 {
                        return Err("window closed");
                    }
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                Sleep(10);
            }
            Err("capture timeout")
        })();
        let mut cleaned = true;
        if old != 0 && old != usize::MAX {
            cleaned &= SelectObject(memory, old) != 0;
        }
        if bitmap != 0 {
            cleaned &= DeleteObject(bitmap) != 0;
        }
        if memory != 0 {
            cleaned &= DeleteDC(memory) != 0;
        }
        if screen != 0 {
            cleaned &= ReleaseDC(window, screen) != 0;
        }
        if window != 0 {
            cleaned &= DestroyWindow(window) != 0;
        }
        result.and(check(cleaned, "GDI cleanup"))
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mainCRTStartup() -> ! {
    let result = run();
    let reported = match result {
        Ok(()) => output(b"GDI FINISHED status=0\r\n"),
        Err(error) => output(b"GDI FAILED api=")
            .and_then(|()| output(error.as_bytes()))
            .and_then(|()| output(b"\r\n")),
    };
    // SAFETY: All probe-owned resources have been released; the exit code reports the outcome.
    unsafe { win32::ExitProcess(u32::from(result.is_err() || reported.is_err())) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    // SAFETY: Terminate rather than unwind across Win32 callbacks.
    unsafe { win32::ExitProcess(250) }
}
