// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

#![no_std]

pub mod arena;
pub mod cosmetic;
pub mod display_modes;
pub mod display_resources;
pub mod driver_info;
pub mod hash;
pub mod ioctl;
pub mod memory;
pub mod modes;
pub mod ring;
pub mod rop;
pub mod slots;

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
