// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Shared display/miniport request codes.

pub const QUERY_AVAILABLE_MODES: u32 = 0x230000;
pub const QUERY_MODE_COUNT: u32 = 0x230004;
pub const QUERY_CURRENT_MODE: u32 = 0x230008;
pub const SET_CURRENT_MODE: u32 = 0x23000c;
pub const RESET_DEVICE: u32 = 0x230010;
pub const MAP_VIDEO_MEMORY: u32 = 0x230058;
pub const UNMAP_VIDEO_MEMORY: u32 = 0x23005c;
pub const GET_INFO: u32 = 0x232000;
pub const SET_CUSTOM_DISPLAY: u32 = 0x232004;
