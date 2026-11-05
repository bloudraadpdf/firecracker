// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::devices::legacy::atapi::AtapiState;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScsiState {
    drives: Vec<(Option<PathBuf>, AtapiState)>,
    sense_size: u32,
    cdb_size: u32,
}

#[cfg(feature = "windows")]
mod device;
#[cfg(feature = "windows")]
pub use device::Scsi;
