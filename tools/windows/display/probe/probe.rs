// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

#![cfg_attr(not(test), no_std)]
#![cfg_attr(all(not(test), target_os = "windows"), no_main)]

pub mod abi;
pub mod model;
#[cfg(all(not(test), target_os = "windows"))]
mod runtime;
