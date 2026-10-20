// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

#![no_std]

mod abi;

#[cfg(any(test, all(windows, any(target_arch = "x86", target_arch = "x86_64"))))]
mod kernel_object;

pub mod arena;
pub mod bitmap;
#[cfg(any(test, all(windows, any(target_arch = "x86", target_arch = "x86_64"))))]
pub mod control;
pub mod cosmetic;
pub mod cursor;
pub mod display_device;
pub mod display_modes;
pub mod display_resources;
pub mod draw;
pub mod driver_info;
pub mod gdi;
#[cfg(any(test, all(windows, any(target_arch = "x86", target_arch = "x86_64"))))]
pub mod hardware;
pub mod hash;
pub mod image;
#[cfg(any(test, all(windows, any(target_arch = "x86", target_arch = "x86_64"))))]
mod image_cache;
pub mod ioctl;
pub mod memory;
pub mod modes;
#[cfg(any(test, all(windows, any(target_arch = "x86", target_arch = "x86_64"))))]
pub mod native_bitmap;
#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
mod native_path;
pub mod paint;
pub mod palette;
pub mod path;
pub mod pdev_config;

#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
pub mod brush;
#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
mod native_pixels;
#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
mod native_raster;
#[cfg(any(test, all(windows, any(target_arch = "x86", target_arch = "x86_64"))))]
pub mod pdev;
#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
pub mod pointer;
pub mod raster_draw;
#[cfg(any(test, all(windows, any(target_arch = "x86", target_arch = "x86_64"))))]
pub mod render;
pub mod ring;
pub mod rop;
#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
mod secondary;
pub mod slots;
pub mod surface;
pub mod text;

#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
mod native_text;

#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
pub mod videoport;

#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
pub mod resources;

#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
pub mod mode_pool;

#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
pub mod adapter;

#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
pub mod miniport;

#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
pub mod engine;

#[cfg(test)]
#[path = "../driver/display_memory.rs"]
mod display_memory;

#[cfg(test)]
mod pointer_tests;
