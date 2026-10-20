// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

#![no_std]

mod abi;

#[cfg(any(test, all(windows, any(target_arch = "x86", target_arch = "x86_64"))))]
mod kernel_object;

pub mod arena;
#[cfg(any(test, all(windows, any(target_arch = "x86", target_arch = "x86_64"))))]
pub mod control;
pub mod cosmetic;
pub mod cursor;
pub mod display_device;
pub mod display_modes;
pub mod display_resources;
pub mod driver_info;
pub mod gdi;
#[cfg(any(test, all(windows, any(target_arch = "x86", target_arch = "x86_64"))))]
pub mod hardware;
pub mod hash;
pub mod ioctl;
pub mod memory;
pub mod modes;
#[cfg(any(test, all(windows, any(target_arch = "x86", target_arch = "x86_64"))))]
pub mod native_bitmap;
pub mod pdev_config;

#[cfg(any(test, all(windows, any(target_arch = "x86", target_arch = "x86_64"))))]
pub mod pdev;
#[cfg(any(test, all(windows, any(target_arch = "x86", target_arch = "x86_64"))))]
pub mod render;
pub mod ring;
pub mod rop;
pub mod slots;
pub mod surface;

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
