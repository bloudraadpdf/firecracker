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
    fn SetDIBColorTable(dc: usize, start: u32, count: u32, colors: *const u32) -> u32;
    fn CreateFontW(
        height: i32,
        width: i32,
        escapement: i32,
        orientation: i32,
        weight: i32,
        italic: u32,
        underline: u32,
        strikeout: u32,
        charset: u32,
        output_precision: u32,
        clip_precision: u32,
        quality: u32,
        pitch: u32,
        face: *const u16,
    ) -> usize;
    fn SetTextColor(dc: usize, color: u32) -> u32;
    fn TextOutW(dc: usize, x: i32, y: i32, text: *const u16, length: i32) -> i32;
    fn GdiAlphaBlend(
        destination: usize,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        source: usize,
        source_x: i32,
        source_y: i32,
        source_width: i32,
        source_height: i32,
        blend: u32,
    ) -> i32;
    fn GdiTransparentBlt(
        destination: usize,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        source: usize,
        source_x: i32,
        source_y: i32,
        source_width: i32,
        source_height: i32,
        color: u32,
    ) -> i32;
    fn StretchBlt(
        destination: usize,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        source: usize,
        source_x: i32,
        source_y: i32,
        source_width: i32,
        source_height: i32,
        rop: u32,
    ) -> i32;
    fn SetStretchBltMode(dc: usize, mode: i32) -> i32;
    fn MaskBlt(
        destination: usize,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        source: usize,
        source_x: i32,
        source_y: i32,
        mask: usize,
        mask_x: i32,
        mask_y: i32,
        rop: u32,
    ) -> i32;
    fn BitBlt(
        destination: usize,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        source: usize,
        source_x: i32,
        source_y: i32,
        rop: u32,
    ) -> i32;
    fn PatBlt(dc: usize, x: i32, y: i32, width: i32, height: i32, rop: u32) -> i32;
    fn CreateCompatibleDC(dc: usize) -> usize;
    fn CreateCompatibleBitmap(dc: usize, width: i32, height: i32) -> usize;
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
    fn CreatePatternBrush(bitmap: usize) -> usize;
    fn CreateHatchBrush(hatch: i32, color: u32) -> usize;
    fn SetBrushOrgEx(dc: usize, x: i32, y: i32, previous: *mut model::Point) -> i32;
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
        for (x, y, source_x, source_y, width, height) in [
            (16, 16, 8, 8, 112, 80),
            (8, 120, 16, 128, 128, 80),
            (160, 240, 168, 240, 128, 64),
            (168, 320, 160, 320, 128, 32),
        ] {
            check(
                BitBlt(dc, x, y, width, height, dc, source_x, source_y, 0x00cc0020) != 0,
                "overlapping BitBlt",
            )?;
        }
        let saved = SaveDC(dc);
        check(saved != 0, "SaveDC copy clip")?;
        let clipped = IntersectClipRect(dc, 21, 23, 121, 91) != 0
            && BitBlt(dc, 16, 16, 112, 80, dc, 8, 8, 0x00cc0020) != 0;
        let restored = RestoreDC(dc, saved) != 0;
        check(clipped && restored, "clipped overlapping BitBlt")?;
        for (index, rop) in [0x00000042, 0x00ff0062, 0x00550009, 0x005a0049]
            .into_iter()
            .enumerate()
        {
            check(
                PatBlt(dc, index as i32 * 64, 352, 48, 24, rop) != 0,
                "PatBlt",
            )?;
        }
        bitmap_copies(dc)?;
        for hatch in 0..6 {
            pattern_rectangle(
                dc,
                CreateHatchBrush(hatch, 0x0037d9e5),
                248,
                96 + hatch * 24,
            )?;
        }
        raster_text(dc)?;
        indexed_copies(dc)?;
        device_bitmap_copies(dc)?;
        check(GdiFlush() != 0, "GdiFlush")
    }
}

unsafe fn pattern_rectangle(dc: usize, brush: usize, x: i32, y: i32) -> Result {
    // SAFETY: the caller owns the brush; its selection ends before deletion.
    unsafe {
        check(brush != 0, "pattern brush")?;
        let saved = SaveDC(dc);
        if saved == 0 {
            DeleteObject(brush);
            return Err("SaveDC pattern");
        }
        let result = (|| {
            let previous = SelectObject(dc, brush);
            check(
                previous != 0 && previous != usize::MAX,
                "SelectObject pattern",
            )?;
            check(SetBrushOrgEx(dc, -3, 5, null_mut()) != 0, "SetBrushOrgEx")?;
            check(SetBkColor(dc, 0x0062ab38) != u32::MAX, "pattern background")?;
            check(SetBkMode(dc, 2) != 0, "pattern opaque")?;
            check(PatBlt(dc, x, y, 40, 16, 0x00f00021) != 0, "pattern copy")?;
            check(
                PatBlt(dc, x + 8, y + 4, 24, 8, 0x005a0049) != 0,
                "pattern xor",
            )?;
            check(GdiFlush() != 0, "GdiFlush pattern")
        })();
        let restored = RestoreDC(dc, saved) != 0;
        let deleted = DeleteObject(brush) != 0;
        result.and(check(restored && deleted, "pattern cleanup"))
    }
}

fn raster_text(dc: usize) -> Result {
    let face = b"Tahoma\0".map(u16::from);
    let text = b"QXL Rust 123".map(u16::from);
    // SAFETY: fonts and DC selections stay live through the flushed draw;
    // RestoreDC removes each font before its deletion.
    unsafe {
        for (index, (quality, background)) in
            [(3, 1), (3, 2), (4, 1), (4, 2)].into_iter().enumerate()
        {
            let saved = SaveDC(dc);
            check(saved != 0, "SaveDC text")?;
            let font = CreateFontW(
                -16,
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                1,
                0,
                0,
                quality,
                0,
                face.as_ptr(),
            );
            let result = (|| {
                check(font != 0, "CreateFontW")?;
                let old = SelectObject(dc, font);
                check(old != 0 && old != usize::MAX, "SelectObject font")?;
                check(SetTextColor(dc, 0x00d72b91) != u32::MAX, "SetTextColor")?;
                check(SetBkColor(dc, 0x0038ab62) != u32::MAX, "SetBkColor text")?;
                check(SetBkMode(dc, background) != 0, "SetBkMode text")?;
                let x = 8 + (index as i32 % 2) * 160;
                let y = 332 + (index as i32 / 2) * 24;
                check(
                    IntersectClipRect(dc, x + 3, y + 2, x + 108, y + 15) != 0,
                    "text clip",
                )?;
                check(
                    TextOutW(dc, x, y, text.as_ptr(), text.len() as i32) != 0,
                    "TextOutW",
                )?;
                check(GdiFlush() != 0, "GdiFlush text")
            })();
            let restored = RestoreDC(dc, saved) != 0;
            let deleted = font == 0 || DeleteObject(font) != 0;
            result.and(check(restored && deleted, "font cleanup"))?;
        }
        Ok(())
    }
}

fn bitmap_copies(dc: usize) -> Result {
    // SAFETY: the caller retains the DC; source DIBs remain selected and owned
    // until all GDI copies are flushed, then their selections are restored.
    unsafe {
        for (row, height) in [32, -32].into_iter().enumerate() {
            let source = CreateCompatibleDC(dc);
            check(source != 0, "CreateCompatibleDC source")?;
            let info = BitmapHeader {
                size: 40,
                width: 37,
                height,
                planes: 1,
                bits: 32,
                compression: 0,
                image_size: 37 * 32 * 4,
                x_pixels: 0,
                y_pixels: 0,
                colors: 0,
                important: 0,
            };
            let mut pixels = null_mut();
            let bitmap = CreateDIBSection(source, &info, 0, &mut pixels, 0, 0);
            let mut previous = 0;
            let result = (|| {
                check(bitmap != 0 && !pixels.is_null(), "CreateDIBSection source")?;
                for y in 0..32usize {
                    for x in 0..37usize {
                        pixels
                            .cast::<u32>()
                            .add(y * 37 + x)
                            .write(if (x + y).is_multiple_of(5) {
                                0x55336699
                            } else {
                                0x55000000
                                    | (((x * 17) as u32 & 255) << 16)
                                    | (((y * 29) as u32 & 255) << 8)
                                    | ((x ^ y) as u32 * 31 & 255)
                            });
                    }
                }
                previous = SelectObject(source, bitmap);
                check(
                    previous != 0 && previous != usize::MAX,
                    "SelectObject source",
                )?;
                let brush = CreatePatternBrush(bitmap);
                pattern_rectangle(dc, brush, 248, 16 + row as i32 * 32)?;
                for (column, rop) in [0x00cc0020, 0x00660046, 0x008800c6, 0x00330008]
                    .into_iter()
                    .enumerate()
                {
                    check(
                        BitBlt(
                            dc,
                            16 + column as i32 * 64,
                            80 + row as i32 * 48,
                            24,
                            20,
                            source,
                            3,
                            5,
                            rop,
                        ) != 0,
                        "bitmap BitBlt",
                    )?;
                }
                raster_operations(dc, source, bitmap, row as i32)?;
                let y = 176 + row as i32 * 48;
                check(
                    GdiTransparentBlt(dc, 16, y, 24, 20, source, 3, 5, 24, 20, 0x00996633) != 0,
                    "GdiTransparentBlt",
                )?;
                let saved = SaveDC(dc);
                check(saved != 0, "SaveDC transparent clip")?;
                let clipped = IntersectClipRect(dc, 84, y + 3, 100, y + 17) != 0
                    && GdiTransparentBlt(dc, 80, y, 24, 20, source, 3, 5, 24, 20, 0x00996633) != 0;
                let restored = RestoreDC(dc, saved) != 0;
                check(clipped && restored, "clipped GdiTransparentBlt")?;
                check(GdiFlush() != 0, "GdiFlush before DIB write")?;
                for y in 0..32usize {
                    for x in 0..37usize {
                        let alpha = ((x * 19 + y * 13) & 255) as u32;
                        let red = (x as u32 * 17 & 255) * alpha / 255;
                        let green = (y as u32 * 29 & 255) * alpha / 255;
                        let blue = ((x ^ y) as u32 * 31 & 255) * alpha / 255;
                        pixels
                            .cast::<u32>()
                            .add(y * 37 + x)
                            .write((alpha << 24) | (red << 16) | (green << 8) | blue);
                    }
                }
                for (column, constant) in [0u8, 64, 128, 255].into_iter().enumerate() {
                    let function = u32::from_le_bytes([0, 0, constant, u8::from(column != 1)]);
                    check(
                        GdiAlphaBlend(
                            dc,
                            16 + column as i32 * 64,
                            272 + row as i32 * 32,
                            24,
                            20,
                            source,
                            3,
                            5,
                            24,
                            20,
                            function,
                        ) != 0,
                        "GdiAlphaBlend",
                    )?;
                }
                check(GdiFlush() != 0, "GdiFlush source")
            })();
            let restored =
                previous == 0 || previous == usize::MAX || SelectObject(source, previous) != 0;
            let deleted = bitmap == 0 || DeleteObject(bitmap) != 0;
            let closed = DeleteDC(source) != 0;
            result.and(check(
                restored && deleted && closed,
                "source bitmap cleanup",
            ))?;
        }
        Ok(())
    }
}

#[repr(C)]
struct MaskInfo {
    header: BitmapHeader,
    colors: [u32; 2],
}

fn raster_operations(dc: usize, source: usize, bitmap: usize, row: i32) -> Result {
    // SAFETY: the caller retains both DCs and the selected source DIB. All
    // temporary selections end before their brush/bitmap owners are deleted.
    unsafe {
        let info = MaskInfo {
            header: BitmapHeader {
                size: 40,
                width: 37,
                height: -32,
                planes: 1,
                bits: 1,
                compression: 0,
                image_size: 8 * 32,
                x_pixels: 0,
                y_pixels: 0,
                colors: 2,
                important: 0,
            },
            colors: [0, 0xffffff],
        };
        let mut bits = null_mut();
        let mask = CreateDIBSection(source, &info.header, 0, &mut bits, 0, 0);
        check(mask != 0 && !bits.is_null(), "CreateDIBSection raster mask")?;
        core::ptr::write_bytes(bits, 0, 8 * 32);
        for y in 0..32usize {
            for x in 0..37usize {
                if (x + 2 * y) % 5 < 2 {
                    let byte = bits.add(y * 8 + x / 8);
                    byte.write(byte.read() | (0x80 >> (x % 8)));
                }
            }
        }
        let saved = SaveDC(dc);
        if saved == 0 {
            DeleteObject(mask);
            return Err("SaveDC raster");
        }
        let brush = if row == 0 {
            CreateSolidBrush(0x00123456)
        } else {
            CreatePatternBrush(bitmap)
        };
        let result = (|| {
            check(brush != 0, "raster brush")?;
            let old = SelectObject(dc, brush);
            check(old != 0 && old != usize::MAX, "SelectObject raster brush")?;
            check(
                SetBrushOrgEx(dc, -3, 5, null_mut()) != 0,
                "raster brush origin",
            )?;
            for (column, rop) in [0x00c000ca, 0x00fb0a09, 0x00bb0226].into_iter().enumerate() {
                check(
                    BitBlt(
                        dc,
                        16 + column as i32 * 64,
                        16 + row * 112,
                        24,
                        20,
                        source,
                        3,
                        5,
                        rop,
                    ) != 0,
                    "ternary BitBlt",
                )?;
            }
            check(SetStretchBltMode(dc, 3) != 0, "SetStretchBltMode")?;
            for (column, width, height) in [(0, 40, 28), (1, 17, 13)] {
                check(
                    StretchBlt(
                        dc,
                        16 + column * 64,
                        40 + row * 112,
                        width,
                        height,
                        source,
                        3,
                        5,
                        24,
                        20,
                        0x00cc0020,
                    ) != 0,
                    "nearest StretchBlt",
                )?;
            }
            let clipped = SaveDC(dc);
            check(clipped != 0, "SaveDC scaled clip")?;
            let ok = IntersectClipRect(dc, 148, 43 + row * 112, 172, 63 + row * 112) != 0
                && StretchBlt(
                    dc,
                    144,
                    40 + row * 112,
                    40,
                    28,
                    source,
                    3,
                    5,
                    24,
                    20,
                    0x00c000ca,
                ) != 0;
            let restored = RestoreDC(dc, clipped) != 0;
            check(ok && restored, "clipped ternary StretchBlt")?;
            for (column, rop) in [0xf0cc0020, 0xccf00021, 0xaacc0020, 0x5ac000ca]
                .into_iter()
                .enumerate()
            {
                check(
                    MaskBlt(
                        dc,
                        16 + column as i32 * 64,
                        72 + row * 112,
                        24,
                        20,
                        source,
                        3,
                        5,
                        mask,
                        1,
                        2,
                        rop,
                    ) != 0,
                    "MaskBlt source/pattern/destination",
                )?;
            }
            check(GdiFlush() != 0, "GdiFlush raster operations")
        })();
        let restored = RestoreDC(dc, saved) != 0;
        let deleted = (brush == 0 || DeleteObject(brush) != 0) && DeleteObject(mask) != 0;
        result.and(check(restored && deleted, "raster object cleanup"))
    }
}

#[repr(C)]
struct IndexedInfo {
    header: BitmapHeader,
    colors: [u32; 256],
}

fn indexed_copies(dc: usize) -> Result {
    // SAFETY: this thread owns the source DC and DIB, restores all selections,
    // and flushes each use before changing colours or deleting its objects.
    unsafe {
        for (case, (depth, height)) in [
            (1u16, 32i32),
            (1, -32),
            (4, 32),
            (4, -32),
            (8, 32),
            (8, -32),
        ]
        .into_iter()
        .enumerate()
        {
            let stride = (37u32 * u32::from(depth)).div_ceil(32) * 4;
            let count = 1u32 << depth;
            let mut colors = [0u32; 256];
            for index in 0..count as usize {
                colors[index] = (((index * 97 + 31) & 255) as u32) << 16
                    | (((index * 53 + 79) & 255) as u32) << 8
                    | ((index * 29 + 113) & 255) as u32;
            }
            let info = IndexedInfo {
                header: BitmapHeader {
                    size: 40,
                    width: 37,
                    height,
                    planes: 1,
                    bits: depth,
                    compression: 0,
                    image_size: stride * 32,
                    x_pixels: 0,
                    y_pixels: 0,
                    colors: count,
                    important: 0,
                },
                colors,
            };
            let source = CreateCompatibleDC(dc);
            check(source != 0, "CreateCompatibleDC indexed")?;
            let mut pixels = null_mut();
            let bitmap = CreateDIBSection(source, &info.header, 0, &mut pixels, 0, 0);
            let mut previous = 0;
            let result = (|| {
                check(bitmap != 0 && !pixels.is_null(), "CreateDIBSection indexed")?;
                pixels.write_bytes(0, stride as usize * 32);
                for y in 0..32usize {
                    for x in 0..37usize {
                        let value = ((x + 3 * y) as u32 & (count - 1)) as u8;
                        let bit = x * depth as usize;
                        let byte = pixels.add(y * stride as usize + bit / 8);
                        byte.write(byte.read() | value << (8 - depth as usize - bit % 8));
                    }
                }
                previous = SelectObject(source, bitmap);
                check(
                    previous != 0 && previous != usize::MAX,
                    "SelectObject indexed",
                )?;
                let y = 8 + case as i32 * 52;
                for (index, rop) in [0x00cc0020, 0x00660046, 0x00cc0020].into_iter().enumerate() {
                    check(
                        BitBlt(dc, 296, y + index as i32 * 12, 17, 9, source, 3, 5, rop) != 0,
                        "indexed BitBlt",
                    )?;
                }
                check(GdiFlush() != 0, "GdiFlush indexed")?;
                let changed = [0x00e32591, 0x0038bc64];
                check(
                    SetDIBColorTable(source, 0, 2, changed.as_ptr()) == 2,
                    "SetDIBColorTable",
                )?;
                check(
                    BitBlt(dc, 296, y + 36, 17, 9, source, 3, 5, 0x00cc0020) != 0,
                    "indexed palette update",
                )?;
                check(GdiFlush() != 0, "GdiFlush palette update")
            })();
            let restored =
                previous == 0 || previous == usize::MAX || SelectObject(source, previous) != 0;
            let deleted = bitmap == 0 || DeleteObject(bitmap) != 0;
            let closed = DeleteDC(source) != 0;
            result.and(check(
                restored && deleted && closed,
                "indexed source cleanup",
            ))?;
        }
        Ok(())
    }
}

fn device_bitmap_copies(dc: usize) -> Result {
    // SAFETY: the caller retains the target DC; all local GDI owners are released
    // after their final flushed operation and restored selection.
    unsafe {
        let source = CreateCompatibleDC(dc);
        check(source != 0, "CreateCompatibleDC device bitmap")?;
        let bitmap = CreateCompatibleBitmap(dc, 32, 24);
        let mut old = 0;
        let result = (|| {
            check(bitmap != 0, "CreateCompatibleBitmap")?;
            old = SelectObject(source, bitmap);
            check(old != 0 && old != usize::MAX, "SelectObject device bitmap")?;
            for (left, color) in [(0, 0x00214ba7), (16, 0x00c97132)] {
                let brush = CreateSolidBrush(color);
                check(brush != 0, "CreateSolidBrush device bitmap")?;
                let filled = FillRect(
                    source,
                    &Rect {
                        left,
                        top: 0,
                        right: left + 16,
                        bottom: 24,
                    },
                    brush,
                ) != 0;
                let deleted = DeleteObject(brush) != 0;
                check(filled && deleted, "FillRect device bitmap")?;
            }
            check(
                BitBlt(source, 3, 4, 25, 16, source, 0, 0, 0x00660046) != 0,
                "overlapping device BitBlt",
            )?;
            check(
                BitBlt(dc, 8, 352, 32, 24, source, 0, 0, 0x00cc0020) != 0,
                "device bitmap to canvas",
            )?;
            check(
                BitBlt(source, 0, 0, 16, 8, dc, 8, 8, 0x00cc0020) != 0,
                "canvas to device bitmap",
            )?;
            check(
                BitBlt(dc, 48, 352, 32, 24, source, 0, 0, 0x00cc0020) != 0,
                "device bitmap round trip",
            )?;
            check(GdiFlush() != 0, "GdiFlush device bitmap")
        })();
        let restored = old == 0 || old == usize::MAX || SelectObject(source, old) != 0;
        let deleted = bitmap == 0 || DeleteObject(bitmap) != 0;
        let closed = DeleteDC(source) != 0;
        result.and(check(
            restored && deleted && closed,
            "device bitmap cleanup",
        ))
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
    let mut retained_dc = 0;
    let mut retained_bitmap = 0;
    let mut retained_old = 0;
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
            retained_dc = CreateCompatibleDC(screen);
            check(retained_dc != 0, "CreateCompatibleDC retained bitmap")?;
            retained_bitmap = CreateCompatibleBitmap(screen, 32, 24);
            check(retained_bitmap != 0, "CreateCompatibleBitmap retained")?;
            retained_old = SelectObject(retained_dc, retained_bitmap);
            check(
                retained_old != 0 && retained_old != usize::MAX,
                "SelectObject retained bitmap",
            )?;
            check(
                BitBlt(retained_dc, 0, 0, 32, 24, screen, 8, 352, 0x00cc0020) != 0
                    && GdiFlush() != 0,
                "retain device bitmap",
            )?;
            let mut expected = [0u32; 32 * 24];
            for (index, pixel) in expected.iter_mut().enumerate() {
                *pixel = bits
                    .cast::<u32>()
                    .add((352 + index / 32) * WIDTH as usize + 8 + index % 32)
                    .read()
                    & 0x00ff_ffff;
            }
            GetAsyncKeyState(0x70);
            output(b"GDI READY x=64 y=64 width=320 height=384; capture then F1\r\n")?;
            let start = GetTickCount();
            while GetTickCount().wrapping_sub(start) < 90000 {
                if GetAsyncKeyState(0x70) & 1 != 0 {
                    check(
                        BitBlt(memory, 8, 352, 32, 24, retained_dc, 0, 0, 0x00cc0020) != 0
                            && GdiFlush() != 0,
                        "read retained device bitmap",
                    )?;
                    for (index, pixel) in expected.iter().enumerate() {
                        let actual = bits
                            .cast::<u32>()
                            .add((352 + index / 32) * WIDTH as usize + 8 + index % 32)
                            .read()
                            & 0x00ff_ffff;
                        check(actual == *pixel, "retained device bitmap pixels")?;
                    }
                    output(b"GDI RETAINED bitmap pixels=768 status=0\r\n")?;
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
        if retained_old != 0 && retained_old != usize::MAX {
            cleaned &= SelectObject(retained_dc, retained_old) != 0;
        }
        if retained_bitmap != 0 {
            cleaned &= DeleteObject(retained_bitmap) != 0;
        }
        if retained_dc != 0 {
            cleaned &= DeleteDC(retained_dc) != 0;
        }
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
