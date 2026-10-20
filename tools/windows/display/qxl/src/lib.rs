// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

#![no_std]

pub mod cosmetic;
pub mod hash;
pub mod modes;
pub mod ring;
pub mod slots;

#[cfg(all(windows, any(target_arch = "x86", target_arch = "x86_64")))]
pub mod videoport;
