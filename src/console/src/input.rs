// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeSet;
use std::io::{self, Write};

use crate::{client, invalid};

pub(crate) struct Motion {
    point: Option<(u16, u16)>,
    next: std::time::Instant,
}

impl Default for Motion {
    fn default() -> Self {
        Self {
            point: None,
            next: std::time::Instant::now(),
        }
    }
}

impl Motion {
    pub fn update(&mut self, point: Option<(u16, u16)>) {
        self.point = point;
    }
    pub fn deadline(&self) -> Option<std::time::Instant> {
        self.point.map(|_| self.next)
    }
    pub fn take(&mut self, now: std::time::Instant) -> Option<(u16, u16)> {
        if now < self.next {
            return None;
        }
        let point = self.point.take()?;
        self.next = now + std::time::Duration::from_micros(16_667);
        Some(point)
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Viewport {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    guest: (u16, u16),
}

impl Viewport {
    pub fn fit(window: (u32, u32), guest: (u16, u16)) -> Option<Self> {
        if window.0 == 0 || window.1 == 0 || guest.0 == 0 || guest.1 == 0 {
            return None;
        }
        let (width, height) = if u64::from(window.0) * u64::from(guest.1)
            > u64::from(window.1) * u64::from(guest.0)
        {
            (
                u32::try_from(u64::from(window.1) * u64::from(guest.0) / u64::from(guest.1))
                    .unwrap()
                    .max(1),
                window.1,
            )
        } else {
            (
                window.0,
                u32::try_from(u64::from(window.0) * u64::from(guest.1) / u64::from(guest.0))
                    .unwrap()
                    .max(1),
            )
        };
        Some(Self {
            x: (window.0 - width) / 2,
            y: (window.1 - height) / 2,
            width,
            height,
            guest,
        })
    }

    pub fn point(self, x: i64, y: i64, dragging: bool) -> Option<(u16, u16)> {
        let x = x.saturating_sub(i64::from(self.x));
        let y = y.saturating_sub(i64::from(self.y));
        if !dragging
            && (x < 0 || y < 0 || x >= i64::from(self.width) || y >= i64::from(self.height))
        {
            return None;
        }
        fn axis(value: i64, extent: u32, guest: u16) -> u16 {
            let max = extent.saturating_sub(1);
            if max == 0 {
                return 0;
            }
            u16::try_from(
                u64::try_from(value.clamp(0, i64::from(max))).unwrap() * u64::from(guest - 1)
                    / u64::from(max),
            )
            .unwrap()
        }
        Some((
            axis(x, self.width, self.guest.0),
            axis(y, self.height, self.guest.1),
        ))
    }
}

#[derive(Default)]
pub(crate) struct Held {
    keys: BTreeSet<u32>,
    buttons: u8,
    point: (u16, u16),
}

impl Held {
    pub fn key(&mut self, stream: &mut impl Write, sym: u32, pressed: bool) -> io::Result<()> {
        if pressed {
            if self.keys.len() >= 256 && !self.keys.contains(&sym) {
                return Err(invalid("too many held keys"));
            }
            self.keys.insert(sym);
        } else if !self.keys.remove(&sym) {
            return Ok(());
        }
        client::key(stream, sym, pressed)
    }
    pub fn pointer(
        &mut self,
        stream: &mut impl Write,
        buttons: u8,
        point: (u16, u16),
    ) -> io::Result<()> {
        self.buttons = buttons;
        self.point = point;
        client::pointer(stream, buttons, point.0, point.1)
    }
    pub fn release(&mut self, stream: &mut impl Write) -> io::Result<()> {
        for key in std::mem::take(&mut self.keys) {
            client::key(stream, key, false)?;
        }
        if self.buttons != 0 {
            self.pointer(stream, 0, self.point)?;
        }
        Ok(())
    }
    pub fn shortcut(&mut self, stream: &mut impl Write, keys: &[u32]) -> io::Result<()> {
        let held = std::mem::take(&mut self.keys);
        for &key in &held {
            client::key(stream, key, false)?;
        }
        for &key in keys {
            client::key(stream, key, true)?;
        }
        for &key in keys.iter().rev() {
            client::key(stream, key, false)?;
        }
        for &key in &held {
            client::key(stream, key, true)?;
        }
        self.keys = held;
        Ok(())
    }
}

#[cfg(feature = "gui")]
pub(crate) fn modifiers(
    state: winit::keyboard::ModifiersState,
    keys: &mut std::collections::HashMap<winit::keyboard::PhysicalKey, u32>,
    fill_missing: bool,
) -> Vec<(u32, bool)> {
    use winit::keyboard::{KeyCode, PhysicalKey};
    let mut changes = Vec::new();
    for (active, left, right, sym) in [
        (
            state.shift_key(),
            KeyCode::ShiftLeft,
            KeyCode::ShiftRight,
            0xffe1,
        ),
        (
            state.control_key(),
            KeyCode::ControlLeft,
            KeyCode::ControlRight,
            0xffe3,
        ),
        (state.alt_key(), KeyCode::AltLeft, KeyCode::AltRight, 0xffe9),
        (
            state.super_key(),
            KeyCode::SuperLeft,
            KeyCode::SuperRight,
            0xffeb,
        ),
    ] {
        let left = PhysicalKey::Code(left);
        let right = PhysicalKey::Code(right);
        if active {
            if fill_missing && !keys.contains_key(&left) && !keys.contains_key(&right) {
                keys.insert(left, sym);
                changes.push((sym, true));
            }
        } else {
            for key in [left, right] {
                if let Some(sym) = keys.remove(&key) {
                    changes.push((sym, false));
                }
            }
        }
    }
    changes
}

#[cfg(feature = "gui")]
pub(crate) fn keysym(event: &winit::event::KeyEvent) -> Option<u32> {
    use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};
    let right = matches!(
        event.physical_key,
        PhysicalKey::Code(
            KeyCode::ShiftRight | KeyCode::ControlRight | KeyCode::AltRight | KeyCode::SuperRight
        )
    );
    let sym = match &event.logical_key {
        Key::Character(s) if s.len() == 1 && s.as_bytes()[0].is_ascii_graphic() => {
            u32::from(s.as_bytes()[0])
        }
        Key::Named(key) => match key {
            NamedKey::Space => 0x20,
            NamedKey::Backspace => 0xff08,
            NamedKey::Tab => 0xff09,
            NamedKey::Enter => 0xff0d,
            NamedKey::Escape => 0xff1b,
            NamedKey::Delete => 0xffff,
            NamedKey::Insert => 0xff63,
            NamedKey::Home => 0xff50,
            NamedKey::End => 0xff57,
            NamedKey::PageUp => 0xff55,
            NamedKey::PageDown => 0xff56,
            NamedKey::ArrowLeft => 0xff51,
            NamedKey::ArrowUp => 0xff52,
            NamedKey::ArrowRight => 0xff53,
            NamedKey::ArrowDown => 0xff54,
            NamedKey::Shift => 0xffe1 + u32::from(right),
            NamedKey::Control => 0xffe3 + u32::from(right),
            NamedKey::Alt => 0xffe9 + u32::from(right),
            NamedKey::Super | NamedKey::Meta => 0xffeb + u32::from(right),
            NamedKey::CapsLock => 0xffe5,
            NamedKey::NumLock => 0xff7f,
            NamedKey::ScrollLock => 0xff14,
            NamedKey::Pause => 0xff13,
            NamedKey::PrintScreen => 0xff61,
            NamedKey::F1 => 0xffbe,
            NamedKey::F2 => 0xffbf,
            NamedKey::F3 => 0xffc0,
            NamedKey::F4 => 0xffc1,
            NamedKey::F5 => 0xffc2,
            NamedKey::F6 => 0xffc3,
            NamedKey::F7 => 0xffc4,
            NamedKey::F8 => 0xffc5,
            NamedKey::F9 => 0xffc6,
            NamedKey::F10 => 0xffc7,
            NamedKey::F11 => 0xffc8,
            NamedKey::F12 => 0xffc9,
            _ => return None,
        },
        _ => return None,
    };
    Some(sym)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn motion_keeps_the_latest_position_without_delaying_button_transitions() {
        let mut motion = Motion::default();
        let now = std::time::Instant::now();
        for x in 0..1000 {
            motion.update(Some((x, x)));
        }
        assert_eq!(motion.take(now), Some((999, 999)));
        motion.update(Some((1, 2)));
        assert_eq!(motion.take(now), None);
        motion.update(Some((3, 4)));
        assert_eq!(
            motion.take(now + std::time::Duration::from_millis(17)),
            Some((3, 4))
        );
        motion.update(Some((5, 6)));
        motion.update(None);
        assert!(motion.deadline().is_none());
        assert_eq!(motion.take(now + std::time::Duration::from_secs(1)), None);
    }
    #[cfg(feature = "gui")]
    #[test]
    fn modifier_flags_without_key_events_are_forwarded_without_duplicating_physical_keys() {
        use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
        let mut keys = std::collections::HashMap::new();
        assert_eq!(
            modifiers(ModifiersState::SUPER, &mut keys, true),
            [(0xffeb, true)]
        );
        assert!(modifiers(ModifiersState::SUPER, &mut keys, true).is_empty());
        assert_eq!(
            modifiers(ModifiersState::empty(), &mut keys, false),
            [(0xffeb, false)]
        );
        keys.insert(PhysicalKey::Code(KeyCode::SuperRight), 0xffec);
        assert!(modifiers(ModifiersState::SUPER, &mut keys, true).is_empty());
        assert_eq!(
            modifiers(ModifiersState::empty(), &mut keys, false),
            [(0xffec, false)]
        );
        assert!(keys.is_empty());
        assert!(modifiers(ModifiersState::SUPER, &mut keys, false).is_empty());
        assert!(keys.is_empty());
    }
    #[test]
    fn scaling_letterboxing_and_dragging_share_exact_edges() {
        let v = Viewport::fit((1000, 1000), (800, 600)).unwrap();
        assert_eq!((v.x, v.y, v.width, v.height), (0, 125, 1000, 750));
        assert_eq!(v.point(0, 125, false), Some((0, 0)));
        assert_eq!(v.point(999, 874, false), Some((799, 599)));
        assert_eq!(v.point(500, 124, false), None);
        assert_eq!(v.point(-10, 1200, true), Some((0, 599)));
        assert!(Viewport::fit((0, 100), (800, 600)).is_none());
        assert_eq!(
            Viewport::fit((1, 1), (1, 1))
                .unwrap()
                .point(i64::MIN, i64::MAX, true),
            Some((0, 0))
        );
        let v = Viewport::fit((1600, 900), (800, 600)).unwrap();
        assert_eq!((v.x, v.y, v.width, v.height), (200, 0, 1200, 900));
    }
    #[test]
    fn focus_release_clears_all_keys_and_buttons_once() {
        let mut held = Held::default();
        let mut out = vec![];
        held.key(&mut out, 0xffeb, true).unwrap();
        held.pointer(&mut out, 1, (799, 599)).unwrap();
        out.clear();
        held.release(&mut out).unwrap();
        assert_eq!(out, [4, 0, 0, 0, 0, 0, 255, 235, 5, 0, 3, 31, 2, 87]);
        out.clear();
        held.release(&mut out).unwrap();
        assert!(out.is_empty());
    }
    #[test]
    fn shortcut_releases_and_restores_host_modifiers() {
        let mut held = Held::default();
        let mut out = vec![];
        held.key(&mut out, 0xffe3, true).unwrap();
        out.clear();
        held.shortcut(&mut out, &[0xffeb]).unwrap();
        assert_eq!(
            out,
            [
                4, 0, 0, 0, 0, 0, 255, 227, 4, 1, 0, 0, 0, 0, 255, 235, 4, 0, 0, 0, 0, 0, 255, 235,
                4, 1, 0, 0, 0, 0, 255, 227
            ]
        );
        assert!(held.keys.contains(&0xffe3));
    }
}
