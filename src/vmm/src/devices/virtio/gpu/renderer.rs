// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::collections::{BTreeMap, BTreeSet};

use vm_memory::{Bytes, GuestMemoryBackend};

use super::protocol::{BackingEntry, Command, MAX_BACKING_ENTRIES, Rect, ResourceId};
use super::state::{Cursor, RendererState, Resource, Scanout};
use crate::devices::display::Frame;
use crate::vstate::memory::{GuestAddress, GuestMemoryMmap};

const MAX_DIMENSION: u32 = 4096;
const MAX_RESOURCE_BYTES: usize = 32 * 1024 * 1024;
const MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;
const MAX_RESOURCES: usize = 64;
const CURSOR_SIZE: u32 = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RendererError {
    #[error("invalid GPU resource")]
    Resource,
    #[error("invalid GPU scanout")]
    Scanout,
    #[error("invalid GPU parameter")]
    Parameter,
    #[error("GPU resource limit exceeded")]
    OutOfMemory,
}

impl RendererError {
    pub fn response_type(self) -> u32 {
        match self {
            Self::OutOfMemory => 0x1201,
            Self::Scanout => 0x1202,
            Self::Resource => 0x1203,
            Self::Parameter => 0x1205,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    NoData,
    DisplayInfo { width: u32, height: u32 },
}

impl Resource {
    fn pixel(&self, x: u32, y: u32) -> u32 {
        let offset = ((y as usize * self.width as usize) + x as usize) * 4;
        let [a, b, c, d] = self.pixels[offset..offset + 4].try_into().unwrap();
        let (red, green, blue, alpha) = match self.format {
            1 => (c, b, a, d),
            2 => (c, b, a, 255),
            3 => (b, c, d, a),
            4 => (b, c, d, 255),
            67 => (a, b, c, d),
            68 => (d, c, b, 255),
            121 => (d, c, b, a),
            134 => (a, b, c, 255),
            _ => unreachable!("format checked at resource creation"),
        };
        u32::from(alpha) << 24 | u32::from(red) << 16 | u32::from(green) << 8 | u32::from(blue)
    }
}

impl Scanout {
    fn flush(&mut self, resource: &Resource, rect: Rect) {
        let left = self.rect.x.max(rect.x);
        let top = self.rect.y.max(rect.y);
        let right = (self.rect.x + self.rect.width).min(rect.x + rect.width);
        let bottom = (self.rect.y + self.rect.height).min(rect.y + rect.height);
        for y in top..bottom {
            for x in left..right {
                let offset = (y - self.rect.y) as usize * self.rect.width as usize
                    + (x - self.rect.x) as usize;
                self.pixels[offset] = resource.pixel(x, y) & 0x00ff_ffff;
            }
        }
    }
}

impl Cursor {
    fn draw(&self, frame: &mut Frame) {
        for y in 0..CURSOR_SIZE {
            let target_y = i64::from(self.y) - i64::from(self.hot_y) + i64::from(y);
            if !(0..i64::from(frame.height)).contains(&target_y) {
                continue;
            }
            for x in 0..CURSOR_SIZE {
                let target_x = i64::from(self.x) - i64::from(self.hot_x) + i64::from(x);
                if !(0..i64::from(frame.width)).contains(&target_x) {
                    continue;
                }
                let source = self.pixels[(y * CURSOR_SIZE + x) as usize];
                let target = &mut frame.pixels[usize::try_from(target_y).unwrap()
                    * usize::from(frame.width)
                    + usize::try_from(target_x).unwrap()];
                let alpha = source >> 24;
                if alpha == 0 {
                    continue;
                }
                let mut blended = 0;
                for shift in [0, 8, 16] {
                    let foreground = source >> shift & 255;
                    let background = *target >> shift & 255;
                    // Cursor colours use premultiplied alpha.
                    blended |=
                        (foreground + (background * (255 - alpha) + 127) / 255).min(255) << shift;
                }
                *target = blended;
            }
        }
    }
}

#[derive(Debug)]
pub struct Renderer {
    resources: BTreeMap<ResourceId, Resource>,
    total_bytes: usize,
    preferred: (u32, u32),
    scanout: Option<Scanout>,
    cursor: Option<Cursor>,
    native: bool,
}

impl Renderer {
    pub fn new(width: u32, height: u32) -> Result<Self, RendererError> {
        pixel_bytes(width, height)?;
        Ok(Self {
            resources: BTreeMap::new(),
            total_bytes: 0,
            preferred: (width, height),
            scanout: None,
            cursor: None,
            native: false,
        })
    }

    pub fn is_native(&self) -> bool {
        self.native
    }

    pub fn save(&self) -> RendererState {
        RendererState {
            resources: self
                .resources
                .iter()
                .map(|(&id, resource)| (id, resource.clone()))
                .collect(),
            preferred: self.preferred,
            scanout: self.scanout.clone(),
            cursor: self.cursor.clone(),
            native: self.native,
        }
    }

    pub fn restore(state: &RendererState, mem: &GuestMemoryMmap) -> Result<Self, RendererError> {
        pixel_bytes(state.preferred.0, state.preferred.1)?;
        if state.resources.len() > MAX_RESOURCES || (!state.native && state.scanout.is_some()) {
            return Err(RendererError::Parameter);
        }
        let mut ids = BTreeSet::new();
        let mut total_bytes = 0;
        for (id, resource) in &state.resources {
            let bytes = pixel_bytes(resource.width, resource.height)?;
            if !ids.insert(*id) || !valid_format(resource.format) || bytes != resource.pixels.len()
            {
                return Err(RendererError::Parameter);
            }
            total_bytes += bytes;
            if total_bytes > MAX_TOTAL_BYTES {
                return Err(RendererError::OutOfMemory);
            }
            if !resource.backing.is_empty() {
                validate_backing(&resource.backing, bytes, mem)?;
            }
        }
        if let Some(scanout) = &state.scanout {
            let resource = state
                .resources
                .iter()
                .find(|(id, _)| *id == scanout.resource)
                .map(|(_, resource)| resource)
                .ok_or(RendererError::Resource)?;
            validate_rect(scanout.rect, resource)?;
            if pixel_bytes(scanout.rect.width, scanout.rect.height)? / 4 != scanout.pixels.len()
                || scanout.pixels.iter().any(|pixel| pixel & 0xff00_0000 != 0)
            {
                return Err(RendererError::Parameter);
            }
        }
        if let Some(cursor) = &state.cursor
            && (cursor.pixels.len() != (CURSOR_SIZE * CURSOR_SIZE) as usize
                || cursor.hot_x >= CURSOR_SIZE
                || cursor.hot_y >= CURSOR_SIZE)
        {
            return Err(RendererError::Parameter);
        }
        Ok(Self {
            resources: state.resources.iter().cloned().collect(),
            total_bytes,
            preferred: state.preferred,
            scanout: state.scanout.clone(),
            cursor: state.cursor.clone(),
            native: state.native,
        })
    }

    pub fn reset(&mut self) {
        self.resources.clear();
        self.total_bytes = 0;
        self.scanout = None;
        self.cursor = None;
        self.native = false;
    }

    pub fn resize(&mut self, width: u32, height: u32) -> Result<bool, RendererError> {
        pixel_bytes(width, height)?;
        let changed = self.preferred != (width, height);
        self.preferred = (width, height);
        Ok(changed)
    }

    pub fn frame(&self) -> Option<Frame> {
        let scanout = self.scanout.as_ref()?;
        let mut frame = Frame {
            width: u16::try_from(scanout.rect.width).unwrap(),
            height: u16::try_from(scanout.rect.height).unwrap(),
            pixels: scanout.pixels.clone(),
        };
        if let Some(cursor) = &self.cursor {
            cursor.draw(&mut frame);
        }
        Some(frame)
    }

    pub fn execute(
        &mut self,
        command: Command<'_>,
        mem: &GuestMemoryMmap,
    ) -> Result<Reply, RendererError> {
        match command {
            Command::DisplayInfo => {
                return Ok(Reply::DisplayInfo {
                    width: self.preferred.0,
                    height: self.preferred.1,
                });
            }
            Command::Create2d {
                id,
                format,
                width,
                height,
            } => {
                let bytes = pixel_bytes(width, height)?;
                if !valid_format(format) {
                    return Err(RendererError::Parameter);
                }
                if self.resources.contains_key(&id) {
                    return Err(RendererError::Resource);
                }
                if self.resources.len() == MAX_RESOURCES
                    || self.total_bytes + bytes > MAX_TOTAL_BYTES
                {
                    return Err(RendererError::OutOfMemory);
                }
                let pixels = zeroed(bytes)?;
                self.resources.insert(
                    id,
                    Resource {
                        width,
                        height,
                        format,
                        pixels,
                        backing: Vec::new(),
                    },
                );
                self.total_bytes += bytes;
            }
            Command::Unref(id) => {
                let resource = self.resources.remove(&id).ok_or(RendererError::Resource)?;
                self.total_bytes -= resource.pixels.len();
                if self
                    .scanout
                    .as_ref()
                    .is_some_and(|scanout| scanout.resource == id)
                {
                    self.scanout = None;
                }
            }
            Command::Attach { id, backing } => {
                let resource = self.resources.get_mut(&id).ok_or(RendererError::Resource)?;
                let mut entries = Vec::new();
                entries
                    .try_reserve_exact(backing.entries().len())
                    .map_err(|_| RendererError::OutOfMemory)?;
                entries.extend(backing.entries());
                validate_backing(&entries, resource.pixels.len(), mem)?;
                if !resource.backing.is_empty() {
                    return Err(RendererError::Parameter);
                }
                resource.backing = entries;
            }
            Command::Detach(id) => self
                .resources
                .get_mut(&id)
                .ok_or(RendererError::Resource)?
                .backing
                .clear(),
            Command::Transfer { rect, offset, id } => {
                let resource = self.resources.get_mut(&id).ok_or(RendererError::Resource)?;
                validate_rect(rect, resource)?;
                if rect.width == 0 || rect.height == 0 {
                    return Ok(Reply::NoData);
                }
                let stride = u64::from(resource.width) * 4;
                let row_bytes = rect.width as usize * 4;
                let last = offset
                    .checked_add(u64::from(rect.height - 1) * stride)
                    .and_then(|value| value.checked_add(row_bytes as u64))
                    .ok_or(RendererError::Parameter)?;
                if last
                    > resource
                        .backing
                        .iter()
                        .map(|entry| u64::from(entry.length))
                        .sum()
                {
                    return Err(RendererError::Parameter);
                }
                for y in 0..rect.height {
                    let target =
                        ((rect.y + y) as usize * resource.width as usize + rect.x as usize) * 4;
                    read_backing(
                        mem,
                        &resource.backing,
                        offset + u64::from(y) * stride,
                        &mut resource.pixels[target..target + row_bytes],
                    )?;
                }
            }
            Command::Scanout {
                rect,
                scanout,
                resource,
            } => {
                if scanout != 0 {
                    return Err(RendererError::Scanout);
                }
                if let Some(id) = resource {
                    let resource = self.resources.get(&id).ok_or(RendererError::Resource)?;
                    validate_rect(rect, resource)?;
                    let bytes = pixel_bytes(rect.width, rect.height)?;
                    let mut scanout = Scanout {
                        resource: id,
                        rect,
                        pixels: zeroed(bytes / 4)?,
                    };
                    scanout.flush(resource, rect);
                    self.scanout = Some(scanout);
                } else {
                    self.scanout = None;
                }
                self.native = true;
            }
            Command::Flush { rect, id } => {
                let resource = self.resources.get(&id).ok_or(RendererError::Resource)?;
                validate_rect(rect, resource)?;
                if let Some(scanout) = &mut self.scanout
                    && scanout.resource == id
                {
                    scanout.flush(resource, rect);
                }
            }
            Command::UpdateCursor {
                scanout,
                x,
                y,
                resource,
                hot_x,
                hot_y,
            } => {
                if scanout != 0 {
                    return Err(RendererError::Scanout);
                }
                self.cursor = if let Some(id) = resource {
                    let resource = self.resources.get(&id).ok_or(RendererError::Resource)?;
                    if (resource.width, resource.height) != (CURSOR_SIZE, CURSOR_SIZE)
                        || hot_x >= CURSOR_SIZE
                        || hot_y >= CURSOR_SIZE
                    {
                        return Err(RendererError::Parameter);
                    }
                    let mut pixels = zeroed((CURSOR_SIZE * CURSOR_SIZE) as usize)?;
                    for y in 0..CURSOR_SIZE {
                        for x in 0..CURSOR_SIZE {
                            pixels[(y * CURSOR_SIZE + x) as usize] = resource.pixel(x, y);
                        }
                    }
                    Some(Cursor {
                        x,
                        y,
                        hot_x,
                        hot_y,
                        pixels,
                    })
                } else {
                    None
                };
            }
            Command::MoveCursor { scanout, x, y } => {
                if scanout != 0 {
                    return Err(RendererError::Scanout);
                }
                if let Some(cursor) = &mut self.cursor {
                    cursor.x = x;
                    cursor.y = y;
                }
            }
        }
        Ok(Reply::NoData)
    }
}

fn pixel_bytes(width: u32, height: u32) -> Result<usize, RendererError> {
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(RendererError::Parameter);
    }
    let bytes = u64::from(width) * u64::from(height) * 4;
    if bytes > MAX_RESOURCE_BYTES as u64 {
        return Err(RendererError::OutOfMemory);
    }
    Ok(usize::try_from(bytes).unwrap())
}

fn valid_format(format: u32) -> bool {
    matches!(format, 1..=4 | 67 | 68 | 121 | 134)
}

fn validate_backing(
    entries: &[BackingEntry],
    bytes: usize,
    mem: &GuestMemoryMmap,
) -> Result<(), RendererError> {
    if entries.is_empty() || entries.len() > MAX_BACKING_ENTRIES {
        return Err(RendererError::Parameter);
    }
    let mut total = 0u64;
    for entry in entries {
        if entry.length == 0 || entry.address.checked_add(u64::from(entry.length)).is_none() {
            return Err(RendererError::Parameter);
        }
        total += u64::from(entry.length);
        if total > MAX_RESOURCE_BYTES as u64 {
            return Err(RendererError::Parameter);
        }
        mem.get_slice(GuestAddress(entry.address), entry.length as usize)
            .map_err(|_| RendererError::Parameter)?;
    }
    if total < bytes as u64 {
        return Err(RendererError::Parameter);
    }
    Ok(())
}

fn validate_rect(rect: Rect, resource: &Resource) -> Result<(), RendererError> {
    if u64::from(rect.x) + u64::from(rect.width) > u64::from(resource.width)
        || u64::from(rect.y) + u64::from(rect.height) > u64::from(resource.height)
    {
        return Err(RendererError::Parameter);
    }
    Ok(())
}

fn zeroed<T: Default + Clone>(length: usize) -> Result<Vec<T>, RendererError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(length)
        .map_err(|_| RendererError::OutOfMemory)?;
    values.resize(length, T::default());
    Ok(values)
}

fn read_backing(
    mem: &GuestMemoryMmap,
    entries: &[BackingEntry],
    mut offset: u64,
    mut target: &mut [u8],
) -> Result<(), RendererError> {
    for entry in entries {
        if offset >= u64::from(entry.length) {
            offset -= u64::from(entry.length);
            continue;
        }
        let count = target
            .len()
            .min(entry.length as usize - usize::try_from(offset).unwrap());
        mem.read_slice(&mut target[..count], GuestAddress(entry.address + offset))
            .map_err(|_| RendererError::Parameter)?;
        target = &mut target[count..];
        if target.is_empty() {
            return Ok(());
        }
        offset = 0;
    }
    Err(RendererError::Parameter)
}

#[cfg(test)]
#[path = "renderer_tests.rs"]
mod tests;
