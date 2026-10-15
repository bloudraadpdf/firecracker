// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Optional protected UEFI variable service.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Private variable-service socket supplied before entering the sandbox.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UefiVarsConfig {
    /// Dedicated helper which owns this VM's authenticated variable store.
    pub socket_path: PathBuf,
}

impl UefiVarsConfig {
    /// Check the pathname Unix socket representation.
    pub fn validate(&self) -> std::io::Result<()> {
        utils::unix_socket::validate(&self.socket_path)
    }
}

/// Explicit destination store generation for an intentional checkpoint restore.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UefiVarsRestoreConfig {
    /// Dedicated destination helper socket.
    pub socket_path: PathBuf,
    /// Current destination generation, checked before replacing its complete state.
    pub expected_generation: u64,
}
