// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0
use super::{Engine, format::*};

pub(super) struct Policy<'a> {
    pub bytes: &'a [u8],
    pub guid: Guid,
    pub name: &'a [u8],
    min: u32,
    max: u32,
    must: u32,
    cant: u32,
    lock: u8,
}
impl<'a> Policy<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, u64> {
        if bytes.len() < 44 || bytes.len() > usize::from(u16::MAX) {
            return Err(INVALID);
        }
        let offset = usize::from(u16_at(bytes, 6)?);
        let lock = bytes[40];
        let min = u32_at(bytes, 24)?;
        let max = u32_at(bytes, 28)?;
        let must = u32_at(bytes, 32)?;
        let cant = u32_at(bytes, 36)?;
        if u32_at(bytes, 0)? != 0x10000
            || usize::from(u16_at(bytes, 4)?) != bytes.len()
            || offset < 44
            || offset > bytes.len()
            || lock > 3
            || min > max
            || must & cant != 0
        {
            return Err(INVALID);
        }
        let name = &bytes[offset..];
        if !name.is_empty() && !valid_name(name, false) {
            return Err(INVALID);
        }
        if lock == 3 {
            if offset < 64 || !valid_name(&bytes[62..offset], false) {
                return Err(INVALID);
            }
        } else if offset != 44 {
            return Err(INVALID);
        }
        Ok(Self {
            bytes,
            guid: number(bytes, 8)?,
            name,
            min,
            max,
            must,
            cant,
            lock,
        })
    }
    fn specificity(&self, guid: Guid, name: &[u8]) -> Option<usize> {
        if self.guid != guid {
            return None;
        }
        if self.name.is_empty() {
            return Some(0);
        }
        if self.name.len() != name.len() {
            return None;
        }
        let mut score = 1;
        for (p, n) in self
            .name
            .as_chunks::<2>()
            .0
            .iter()
            .zip(name.as_chunks::<2>().0.iter())
        {
            let (p, n) = (u16::from_le_bytes(*p), u16::from_le_bytes(*n));
            if p == u16::from(b'#') {
                if !matches!(n, 48..=57 | 65..=70 | 97..=102) {
                    return None;
                }
            } else if p != n {
                return None;
            } else {
                score += 1;
            }
        }
        Some(score)
    }
    fn locked(&self, engine: &Engine, guid: Guid, name: &[u8]) -> Result<bool, u64> {
        Ok(match self.lock {
            0 => false,
            1 => true,
            2 => engine.find(guid, name).is_some(),
            3 => engine
                .find(
                    number(self.bytes, 44)?,
                    &self.bytes[62..usize::from(u16_at(self.bytes, 6)?)],
                )
                .is_some_and(|v| v.data == [self.bytes[60]]),
            _ => unreachable!(),
        })
    }
}
impl Engine {
    pub(super) fn matching_policy(&self, guid: Guid, name: &[u8]) -> Option<Policy<'_>> {
        self.state
            .policies
            .iter()
            .rev()
            .filter_map(|p| Policy::parse(p).ok())
            .filter_map(|p| p.specificity(guid, name).map(|score| (score, p)))
            .max_by_key(|(score, _)| *score)
            .map(|(_, p)| p)
    }
    pub(super) fn check_policy(
        &self,
        guid: Guid,
        name: &[u8],
        attributes: u32,
        size: usize,
        deleting: bool,
    ) -> Status {
        if self.state.flags & 7 == 0 {
            return Ok(());
        }
        let Some(policy) = self.matching_policy(guid, name) else {
            return Ok(());
        };
        if policy.locked(self, guid, name)? {
            return Err(PROTECTED);
        }
        if !deleting
            && (size < usize::try_from(policy.min).unwrap()
                || size > usize::try_from(policy.max).unwrap()
                || attributes & policy.must != policy.must
                || attributes & policy.cant != 0)
        {
            return Err(INVALID);
        }
        Ok(())
    }
    pub(super) fn register_policy(&mut self, bytes: &[u8]) -> Status {
        let policy = Policy::parse(bytes)?;
        if self.state.flags & 8 != 0 {
            return Err(PROTECTED);
        }
        if self
            .state
            .policies
            .iter()
            .filter_map(|p| Policy::parse(p).ok())
            .any(|p| p.guid == policy.guid && p.name == policy.name)
        {
            return Err(ALREADY);
        }
        if self.state.policies.len() == 4096 || self.used() + bytes.len() > STORAGE {
            return Err(NO_SPACE);
        }
        self.state.policies.push(bytes.to_vec());
        Ok(())
    }
    pub(super) fn lock_variable(&mut self, guid: Guid, name: &[u8]) -> Status {
        if !valid_name(name, false) || 44 + name.len() > usize::from(u16::MAX) {
            return Err(INVALID);
        }
        let mut policy = vec![0; 44];
        policy[..4].copy_from_slice(&0x10000u32.to_le_bytes());
        policy[4..6].copy_from_slice(&u16::try_from(44 + name.len()).unwrap().to_le_bytes());
        policy[6..8].copy_from_slice(&44u16.to_le_bytes());
        policy[8..24].copy_from_slice(&guid);
        policy[28..32].copy_from_slice(&u32::MAX.to_le_bytes());
        policy[40] = 1;
        policy.extend(name);
        self.register_policy(&policy)
    }
}
