// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};

use super::protocol::{BackingEntry, Rect, ResourceId};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Resource {
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub pixels: Vec<u8>,
    pub backing: Vec<BackingEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Scanout {
    pub resource: ResourceId,
    pub rect: Rect,
    pub pixels: Vec<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Cursor {
    pub x: u32,
    pub y: u32,
    pub hot_x: u32,
    pub hot_y: u32,
    pub pixels: Vec<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RendererState {
    pub(super) resources: Vec<(ResourceId, Resource)>,
    pub(super) preferred: (u32, u32),
    pub(super) scanout: Option<Scanout>,
    pub(super) cursor: Option<Cursor>,
    pub(super) native: bool,
}
