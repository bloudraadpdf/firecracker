// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

//! Optional TPM backend supplied by the launcher.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Private control socket of a dedicated swtpm instance.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TpmConfig {
    /// Connect before entering the sandbox; Firecracker never launches the helper.
    pub socket_path: PathBuf,
}

impl TpmConfig {
    /// Reject paths that cannot be represented by a pathname Unix socket.
    pub fn validate(&self) -> std::io::Result<()> {
        super::require_windows_support()?;
        utils::unix_socket::validate(&self.socket_path)
    }
}
