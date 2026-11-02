// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Shared display/miniport request codes.

pub const QUERY_AVAILABLE_MODES: u32 = 0x230400;
pub const QUERY_MODE_COUNT: u32 = 0x230404;
pub const QUERY_CURRENT_MODE: u32 = 0x230408;
pub const SET_CURRENT_MODE: u32 = 0x23040c;
pub const RESET_DEVICE: u32 = 0x230410;
pub const MAP_VIDEO_MEMORY: u32 = 0x230458;
pub const UNMAP_VIDEO_MEMORY: u32 = 0x23045c;
pub const GET_INFO: u32 = 0x232000;
pub const SET_CUSTOM_DISPLAY: u32 = 0x232004;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_video_requests_match_ntddvdeo_function_numbers() {
        for (request, function) in [
            (QUERY_AVAILABLE_MODES, 0x100),
            (QUERY_MODE_COUNT, 0x101),
            (QUERY_CURRENT_MODE, 0x102),
            (SET_CURRENT_MODE, 0x103),
            (RESET_DEVICE, 0x104),
            (MAP_VIDEO_MEMORY, 0x116),
            (UNMAP_VIDEO_MEMORY, 0x117),
        ] {
            assert_eq!(request, (0x23 << 16) | (function << 2));
        }
    }
}
