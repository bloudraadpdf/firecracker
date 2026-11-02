// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::fmt;
use std::ops::Range;

use serde::de::{Error, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use super::protocol::{BackingEntry, MAX_BACKING_ENTRIES, Rect, ResourceId};

pub(super) const MAX_RESOURCE_BYTES: usize = 32 * 1024 * 1024;
pub(super) const MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;
pub(super) const MAX_RESOURCES: usize = 64;
const MAX_BACKING: usize = MAX_RESOURCES * MAX_BACKING_ENTRIES;
const MAX_SCANOUT_PIXELS: usize = MAX_RESOURCE_BYTES / 4;
const CURSOR_PIXELS: usize = 64 * 64;

#[cfg(feature = "windows")]
#[derive(Clone, Debug)]
pub(super) struct Resource {
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub pixels: Vec<u8>,
    pub backing: Vec<BackingEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct ResourceState {
    pub id: ResourceId,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub pixels: Range<usize>,
    pub backing: Range<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Scanout {
    pub resource: ResourceId,
    pub rect: Rect,
    #[serde(deserialize_with = "decode_vec::<_, _, MAX_SCANOUT_PIXELS>")]
    pub pixels: Vec<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Cursor {
    pub x: u32,
    pub y: u32,
    pub hot_x: u32,
    pub hot_y: u32,
    #[serde(deserialize_with = "decode_vec::<_, _, CURSOR_PIXELS>")]
    pub pixels: Vec<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RendererState {
    // Flat payloads let the decoder bound aggregate allocation before reading pixels.
    #[serde(deserialize_with = "decode_vec::<_, _, MAX_RESOURCES>")]
    pub(super) resources: Vec<ResourceState>,
    #[serde(deserialize_with = "decode_vec::<_, _, MAX_TOTAL_BYTES>")]
    pub(super) pixels: Vec<u8>,
    #[serde(deserialize_with = "decode_vec::<_, _, MAX_BACKING>")]
    pub(super) backing: Vec<BackingEntry>,
    pub(super) preferred: (u32, u32),
    pub(super) scanout: Option<Scanout>,
    pub(super) cursor: Option<Cursor>,
    pub(super) native: bool,
}

fn decode_vec<'de, D, T, const MAX: usize>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Bounded<T, const MAX: usize>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>, const MAX: usize> Visitor<'de> for Bounded<T, MAX> {
        type Value = Vec<T>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            write!(formatter, "at most {MAX} GPU state elements")
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
            let length = sequence.size_hint().unwrap_or(0);
            if length > MAX {
                return Err(A::Error::custom("GPU state sequence exceeds limit"));
            }
            let mut values = Vec::new();
            values.try_reserve_exact(length).map_err(A::Error::custom)?;
            while let Some(value) = sequence.next_element()? {
                if values.len() == MAX {
                    return Err(A::Error::custom("GPU state sequence exceeds limit"));
                }
                values.try_reserve(1).map_err(A::Error::custom)?;
                values.push(value);
            }
            Ok(values)
        }
    }
    deserializer.deserialize_seq(Bounded::<T, MAX>(std::marker::PhantomData))
}
