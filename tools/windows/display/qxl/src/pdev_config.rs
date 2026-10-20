// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! PDev mode selection and native GDI capabilities from display/driver.c.

use core::ffi::c_void;
use core::mem::{offset_of, size_of};

use crate::abi::record;
#[cfg(test)]
use crate::display_modes::DEVMODE_BYTES;
use crate::display_modes::supported;
use crate::modes::VideoModeInformation;
use crate::surface::{Format, Layout};

const GCAPS_ARBRUSHOPAQUE: u32 = 1 << 15;
const GCAPS_ARBRUSHTEXT: u32 = 1 << 28;
const GCAPS_GRAY16: u32 = 1 << 24;
const GCAPS_OPAQUERECT: u32 = 1 << 8;
const GCAPS_WINDINGFILL: u32 = 1 << 3;
const GCAPS2_MOUSETRAILS: u32 = 1 << 9;
const GCAPS2_ALPHACURSOR: u32 = 1 << 5;

record! { LogFont {
    height: i32, width: i32, escapement: i32, orientation: i32, weight: i32,
    italic: u8, underline: u8, strike_out: u8, charset: u8,
    output_precision: u8, clip_precision: u8, quality: u8, pitch: u8,
    face: [u16; 32],
}}

record! { ColorInfo {
    primaries: [[i32; 3]; 7],
    gamma: [i32; 3],
    dye_correction: [i32; 6],
}}

record! { GdiInfo {
    version: u32, technology: u32, width_mm: u32, height_mm: u32,
    width: u32, height: u32, bits_per_pixel: u32, planes: u32,
    colors: u32, raster: u32, log_pixels_x: u32, log_pixels_y: u32,
    text_caps: u32, dac_red: u32, dac_green: u32, dac_blue: u32,
    aspect_x: u32, aspect_y: u32, aspect_xy: u32,
    style_step_x: i32, style_step_y: i32, style_density: i32,
    physical_offset: [i32; 2], physical_size: [i32; 2], palette_registers: u32,
    color: ColorInfo, device_dpi: u32, primary_order: u32, ht_pattern_size: u32,
    ht_output_format: u32, ht_flags: u32, refresh: u32, blit_alignment: u32,
    panning_width: u32, panning_height: u32, panning_alignment_x: u32,
    panning_alignment_y: u32, ht_width: u32, ht_height: u32,
    ht_pattern_a: *mut u8, ht_pattern_b: *mut u8, ht_pattern_c: *mut u8,
    shade_blend: u32, physical_pixel_characteristics: u32, physical_pixel_gamma: u32,
}}

record! { DeviceInfo {
    graphics_caps: u32,
    fonts: [LogFont; 3],
    font_count: u32,
    dither_format: u32,
    dither_width: u16,
    dither_height: u16,
    palette: *mut c_void,
    graphics_caps2: u32,
}}

const _: () = {
    assert!(size_of::<LogFont>() == 92);
    assert!(offset_of!(LogFont, face) == 28);
    assert!(size_of::<ColorInfo>() == 120);
    assert!(offset_of!(GdiInfo, color) == 108);
    assert!(offset_of!(GdiInfo, ht_pattern_a) == 280);
    assert!(offset_of!(DeviceInfo, dither_format) == 284);
};
#[cfg(target_pointer_width = "32")]
const _: () = {
    assert!(size_of::<GdiInfo>() == 304);
    assert!(size_of::<DeviceInfo>() == 300);
    assert!(offset_of!(DeviceInfo, palette) == 292);
};
#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(size_of::<GdiInfo>() == 320);
    assert!(size_of::<DeviceInfo>() == 312);
    assert!(offset_of!(DeviceInfo, palette) == 296);
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidRequest,
    NoMatchingMode,
    InvalidMode,
}

#[derive(Clone, Copy, Debug)]
pub struct Request {
    width: u32,
    height: u32,
    depth: u32,
    frequency: u32,
    rotation: u32,
    log_pixels: u16,
}

impl Request {
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        let header = bytes.get(68..70).ok_or(Error::InvalidRequest)?;
        let size = u16::from_le_bytes([header[0], header[1]]) as usize;
        if size < 188 || size > bytes.len() {
            return Err(Error::InvalidRequest);
        }
        let field = |offset| {
            u32::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ])
        };
        let result = Self {
            width: field(172),
            height: field(176),
            depth: field(168),
            frequency: field(184),
            rotation: field(84),
            log_pixels: u16::from_le_bytes([bytes[166], bytes[167]]),
        };
        if result.width == 0
            || result.height == 0
            || !matches!(result.depth, 16 | 32)
            || result.rotation > 3
        {
            return Err(Error::InvalidRequest);
        }
        Ok(result)
    }

    pub fn select(
        self,
        modes: impl IntoIterator<Item = VideoModeInformation>,
    ) -> Result<Configuration, Error> {
        let mode = modes
            .into_iter()
            .find(|mode| {
                supported(mode)
                    && mode.width == self.width
                    && mode.height == self.height
                    && mode.bits_per_plane == self.depth
                    && mode.frequency == self.frequency
                    && mode.driver_attributes == self.rotation
            })
            .ok_or(Error::NoMatchingMode)?;
        Configuration::new(mode, self.log_pixels)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Configuration {
    pub mode: VideoModeInformation,
    pub layout: Layout,
    pub max_bitmap_pixels: u32,
    pub gdi: GdiInfo,
    pub device: DeviceInfo,
}

impl Configuration {
    fn new(mode: VideoModeInformation, log_pixels: u16) -> Result<Self, Error> {
        let format = match mode.bits_per_plane {
            16 => Format::Rgb555,
            32 => Format::Xrgb8888,
            _ => return Err(Error::InvalidMode),
        };
        let layout = Layout::with_stride(mode.width, mode.height, format, mode.stride)
            .map_err(|_| Error::InvalidMode)?;
        let pixels = mode
            .width
            .checked_mul(mode.height)
            .ok_or(Error::InvalidMode)?;
        let max_bitmap_pixels = pixels.checked_add(pixels / 2).ok_or(Error::InvalidMode)?;
        let gdi = GdiInfo {
            version: 0x5001,
            technology: 1,
            width_mm: mode.width_mm,
            height_mm: mode.height_mm,
            width: mode.width,
            height: mode.height,
            bits_per_pixel: mode.bits_per_plane,
            planes: mode.planes,
            colors: u32::MAX,
            log_pixels_x: u32::from(log_pixels),
            log_pixels_y: u32::from(log_pixels),
            text_caps: 8192,
            dac_red: mode.red_bits,
            dac_green: mode.green_bits,
            dac_blue: mode.blue_bits,
            aspect_x: 36,
            aspect_y: 36,
            aspect_xy: 51,
            style_step_x: 1,
            style_step_y: 1,
            style_density: 3,
            color: ColorInfo {
                primaries: [
                    [6700, 3300, 0],
                    [2100, 7100, 0],
                    [1400, 800, 0],
                    [1750, 3950, 0],
                    [4050, 2050, 0],
                    [4400, 5200, 0],
                    [3127, 3290, 0],
                ],
                gamma: [20000; 3],
                ..ColorInfo::default()
            },
            primary_order: 4,
            ht_pattern_size: 3,
            ht_output_format: match format {
                Format::Rgb555 => 5,
                Format::Xrgb8888 => 7,
            },
            ht_flags: 4,
            refresh: mode.frequency,
            ..GdiInfo::default()
        };
        let device = DeviceInfo {
            graphics_caps: GCAPS_ARBRUSHOPAQUE
                | GCAPS_ARBRUSHTEXT
                | GCAPS_GRAY16
                | GCAPS_OPAQUERECT
                | GCAPS_WINDINGFILL,
            fonts: [
                font(16, 7, 700, 0, 0, 2, "System"),
                font(12, 9, 400, 2, 2, 2, "MS Sans Serif"),
                font(12, 9, 400, 2, 2, 1, "Courier"),
            ],
            dither_format: format.bitmap_format(),
            graphics_caps2: GCAPS2_MOUSETRAILS | GCAPS2_ALPHACURSOR,
            ..DeviceInfo::default()
        };
        Ok(Self {
            mode,
            layout,
            max_bitmap_pixels,
            gdi,
            device,
        })
    }
}

fn font(
    height: i32,
    width: i32,
    weight: i32,
    clip_precision: u8,
    quality: u8,
    pitch: u8,
    name: &str,
) -> LogFont {
    let mut result = LogFont {
        height,
        width,
        weight,
        clip_precision,
        quality,
        pitch,
        ..LogFont::default()
    };
    for (output, character) in result.face.iter_mut().zip(name.encode_utf16()) {
        *output = character;
    }
    result
}

#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
mod native {
    use super::*;
    use core::ptr::NonNull;

    use crate::engine::Modes;
    use crate::gdi::{self, Palette};

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum PrepareError {
        Modes(u32),
        Mode(Error),
        Palette(gdi::Error),
    }

    #[derive(Debug)]
    pub struct PreparedDevice {
        configuration: Configuration,
        palette: Palette,
    }

    impl PreparedDevice {
        /// # Safety
        /// Call with a live miniport handle in a serialised, passive-level GDI
        /// callback. All access and destruction remain at PASSIVE_LEVEL. Retain
        /// this owner while GDI uses its palette handle in device information.
        pub unsafe fn new(driver: NonNull<c_void>, devmode: &[u8]) -> Result<Self, PrepareError> {
            let request = Request::parse(devmode).map_err(PrepareError::Mode)?;

            let configuration = {
                // SAFETY: the caller supplies the live handle and GDI context.
                let modes = unsafe { Modes::query(driver) }.map_err(PrepareError::Modes)?;

                request
                    .select(modes.entries())
                    .map_err(PrepareError::Mode)?
            };

            let mode = configuration.mode;
            // SAFETY: configuration selected a supported true-colour mode;
            // this callback and the retained owner's teardown are passive-level.
            let palette = unsafe { Palette::new([mode.red_mask, mode.green_mask, mode.blue_mask]) }
                .map_err(PrepareError::Palette)?;
            Ok(Self {
                configuration,
                palette,
            })
        }

        pub fn configuration(&self) -> &Configuration {
            &self.configuration
        }

        pub fn device_info(&self) -> DeviceInfo {
            DeviceInfo {
                palette: self.palette.handle(),
                ..self.configuration.device
            }
        }
    }
}

#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
pub use native::{PrepareError, PreparedDevice};

#[cfg(test)]
mod tests {
    use super::*;

    fn request(mode: &VideoModeInformation) -> [u8; DEVMODE_BYTES] {
        let mut bytes = [0; DEVMODE_BYTES];
        bytes[68..70].copy_from_slice(&(DEVMODE_BYTES as u16).to_le_bytes());
        bytes[84..88].copy_from_slice(&mode.driver_attributes.to_le_bytes());
        bytes[166..168].copy_from_slice(&120u16.to_le_bytes());
        bytes[168..172].copy_from_slice(&mode.bits_per_plane.to_le_bytes());
        bytes[172..176].copy_from_slice(&mode.width.to_le_bytes());
        bytes[176..180].copy_from_slice(&mode.height.to_le_bytes());
        bytes[184..188].copy_from_slice(&mode.frequency.to_le_bytes());
        bytes
    }

    #[test]
    fn exact_selection_preserves_mode_id_padding_and_rotation() {
        let mut chosen = VideoModeInformation::custom(91, 800, 600, 32).unwrap();
        chosen.stride = 3248;
        chosen.driver_attributes = 3;
        let mut candidates = [chosen; 5];
        candidates[0].width += 1;
        candidates[1].frequency += 1;
        candidates[2].driver_attributes = 0;
        candidates[3].bits_per_plane = 16;
        let configuration = Request::parse(&request(&chosen))
            .unwrap()
            .select(candidates)
            .unwrap();
        assert_eq!(configuration.mode, chosen);
        assert_eq!(configuration.layout.stride(), 3248);
        assert_eq!(configuration.layout.bytes(), 3248 * 600);
        assert_eq!(configuration.max_bitmap_pixels, 720000);
        assert_eq!(configuration.gdi.log_pixels_x, 120);
        assert_eq!(configuration.gdi.log_pixels_y, 120);
        assert_eq!(configuration.device.dither_format, 6);
        assert_eq!(configuration.gdi.ht_output_format, 7);
        assert_eq!(configuration.gdi.style_density, 3);
    }

    #[test]
    fn malformed_requests_and_unmatched_modes_fail_without_configuration() {
        let valid = VideoModeInformation::custom(1, 640, 480, 16).unwrap();
        let bytes = request(&valid);
        assert!(matches!(
            Request::parse(&bytes[..219]),
            Err(Error::InvalidRequest)
        ));
        assert!(matches!(
            Request::parse(&bytes[..69]),
            Err(Error::InvalidRequest)
        ));
        let mut short = bytes;
        short[68..70].copy_from_slice(&187u16.to_le_bytes());
        assert!(matches!(Request::parse(&short), Err(Error::InvalidRequest)));
        let mut invalid = bytes;
        invalid[84..88].copy_from_slice(&4u32.to_le_bytes());
        assert!(matches!(
            Request::parse(&invalid),
            Err(Error::InvalidRequest)
        ));
        assert!(matches!(
            Request::parse(&bytes).unwrap().select([]),
            Err(Error::NoMatchingMode)
        ));
        let mut unaligned = valid;
        unaligned.stride += 1;
        assert!(matches!(
            Request::parse(&bytes).unwrap().select([unaligned]),
            Err(Error::NoMatchingMode)
        ));
    }

    #[test]
    fn mode_prefix_does_not_require_trailing_icm_fields() {
        let mode = VideoModeInformation::custom(1, 800, 600, 32).unwrap();
        let mut bytes = request(&mode);
        bytes[68..70].copy_from_slice(&188u16.to_le_bytes());
        assert_eq!(
            Request::parse(&bytes[..188])
                .unwrap()
                .select([mode])
                .unwrap()
                .mode,
            mode
        );
    }

    #[test]
    fn capability_encoding_preserves_fonts_masks_and_zeroes_native_padding() {
        let mode = VideoModeInformation::custom(1, 640, 480, 16).unwrap();
        let mut configuration = Request::parse(&request(&mode))
            .unwrap()
            .select([mode])
            .unwrap();
        assert_eq!(
            (
                configuration.mode.red_mask,
                configuration.mode.green_mask,
                configuration.mode.blue_mask
            ),
            (0x7c00, 0x3e0, 0x1f)
        );
        assert_eq!(configuration.device.graphics_caps & (1 << 12), 0);
        assert_eq!(configuration.device.dither_format, 4);
        assert_eq!(configuration.gdi.ht_output_format, 5);
        assert_eq!(configuration.device.fonts[0].height, 16);
        assert_eq!(configuration.device.fonts[0].weight, 700);
        assert_eq!(configuration.device.fonts[2].pitch, 1);
        assert_eq!(configuration.gdi.color.gamma, [20000; 3]);
        configuration.device.palette = core::ptr::without_provenance_mut(0x12345678);
        let mut bytes = [0xa5; size_of::<DeviceInfo>() + 4];
        assert_eq!(
            configuration.device.encode(&mut bytes),
            Some(size_of::<DeviceInfo>())
        );
        assert_eq!(u32::from_le_bytes(bytes[284..288].try_into().unwrap()), 4);
        assert_eq!(&bytes[32..46], b"S\0y\0s\0t\0e\0m\0\0\0");
        let palette = offset_of!(DeviceInfo, palette);
        assert!(bytes[292..palette].iter().all(|byte| *byte == 0));
        assert_eq!(
            &bytes[palette..palette + size_of::<usize>()],
            &0x12345678usize.to_le_bytes()
        );
        let end = offset_of!(DeviceInfo, graphics_caps2) + 4;
        assert!(
            bytes[end..size_of::<DeviceInfo>()]
                .iter()
                .all(|byte| *byte == 0)
        );
        assert_eq!(&bytes[size_of::<DeviceInfo>()..], &[0xa5; 4]);
        let mut gdi = [0xa5; size_of::<GdiInfo>()];
        configuration.gdi.encode(&mut gdi).unwrap();
        assert_eq!(&gdi[16..24], &[128, 2, 0, 0, 224, 1, 0, 0]);
        assert!(
            gdi[280..280 + 3 * size_of::<usize>()]
                .iter()
                .all(|byte| *byte == 0)
        );
        let mut short = [0xa5; 8];
        assert_eq!(configuration.gdi.encode(&mut short), None);
        assert_eq!(configuration.device.encode(&mut short), None);
        assert_eq!(short, [0xa5; 8]);
    }
}
