// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0
use super::{Engine, format::*};

impl Engine {
    pub fn transact(&mut self, buffer: &mut [u8]) -> Result<u32, &'static str> {
        if !(24..=MAX_FRAME).contains(&buffer.len()) {
            return Err("invalid MM buffer size");
        }
        if size_at(buffer, 16) != Ok(buffer.len() - 24) {
            return Ok(0x12);
        }
        let guid: Guid = number(buffer, 0).unwrap();
        match guid {
            VARIABLE_PROTOCOL => {
                if buffer.len() < 40 {
                    return Ok(0x12);
                }
                let status = self.variable_request(buffer).err().unwrap_or(0);
                put64(buffer, 32, status);
            }
            POLICY_PROTOCOL => {
                if buffer.len() < 44 {
                    return Ok(0x12);
                }
                let status = self.policy_request(buffer).err().unwrap_or(0);
                put64(buffer, 36, status);
            }
            END_DXE | READY | EXIT => {
                if buffer.len() != 24 {
                    return Ok(0x12);
                }
                self.state.flags |= match guid {
                    END_DXE => 1,
                    READY => 2,
                    _ => 4,
                };
            }
            _ => return Ok(0x11),
        }
        Ok(0)
    }
    fn variable_request(&mut self, buffer: &mut [u8]) -> Status {
        match u64_at(buffer, 24)? {
            1 | 3 => {
                let guid = number(buffer, 40)?;
                let capacity = size_at(buffer, 56)?;
                let name_size = size_at(buffer, 64)?;
                let attributes = u32_at(buffer, 72)?;
                let name = bounded_range(buffer, 76, name_size)?.to_vec();
                if !valid_name(&name, false) {
                    return Err(INVALID);
                }
                let offset = 76 + name_size;
                if u64_at(buffer, 24)? == 3 {
                    self.update(
                        guid,
                        &name,
                        attributes,
                        bounded_range(buffer, offset, capacity)?,
                    )
                } else {
                    let value = self
                        .find(guid, &name)
                        .filter(|v| v.visible(self.runtime()))
                        .ok_or(NOT_FOUND)?;
                    bounded_range(buffer, offset, capacity)?;
                    put64(buffer, 56, u64::try_from(value.data.len()).unwrap());
                    buffer[72..76].copy_from_slice(&value.attributes.to_le_bytes());
                    if capacity < value.data.len() {
                        return Err(TOO_SMALL);
                    }
                    buffer[offset..offset + value.data.len()].copy_from_slice(&value.data);
                    Ok(())
                }
            }
            2 => {
                let guid = number(buffer, 40)?;
                let capacity = size_at(buffer, 56)?;
                let input = bounded_range(buffer, 64, capacity)?;
                let end = input
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .position(|u| *u == [0, 0])
                    .map(|p| (p + 1) * 2)
                    .ok_or(INVALID)?;
                let name = &input[..end];
                if !valid_name(name, true) {
                    return Err(INVALID);
                }
                let variables = &self.state.variables;
                let start = if name == [0, 0] {
                    0
                } else {
                    variables
                        .iter()
                        .position(|v| v.matches(guid, name) && v.visible(self.runtime()))
                        .ok_or(NOT_FOUND)?
                        + 1
                };
                let next = variables[start..]
                    .iter()
                    .find(|v| v.visible(self.runtime()))
                    .ok_or(NOT_FOUND)?;
                put64(buffer, 56, u64::try_from(next.name.len()).unwrap());
                if capacity < next.name.len() {
                    return Err(TOO_SMALL);
                }
                buffer[40..56].copy_from_slice(&next.guid);
                buffer[64..64 + next.name.len()].copy_from_slice(&next.name);
                Ok(())
            }
            4 => {
                let attributes = u32_at(buffer, 64)? & !APPEND;
                if !super::valid_attributes(attributes) {
                    return Err(INVALID);
                }
                if self.runtime() && attributes & RT == 0 {
                    return Err(INVALID);
                }
                put64(buffer, 40, u64::try_from(STORAGE).unwrap());
                put64(
                    buffer,
                    48,
                    u64::try_from(STORAGE.saturating_sub(self.used())).unwrap(),
                );
                let digest = if attributes & AUTH != 0 { 32 } else { 0 };
                put64(
                    buffer,
                    56,
                    u64::try_from(STORAGE - VARIABLE_OVERHEAD - digest).unwrap(),
                );
                Ok(())
            }
            5 | 6 => {
                self.state.flags |= if u64_at(buffer, 24)? == 5 { 2 } else { 4 };
                Ok(())
            }
            8 => {
                let guid = number(buffer, 40)?;
                let name_size = size_at(buffer, 56)?;
                self.lock_variable(guid, bounded_range(buffer, 64, name_size)?)
            }
            11 => {
                bounded_range(buffer, 40, 8)?;
                put64(buffer, 40, u64::try_from(MAX_FRAME - 40).unwrap());
                Ok(())
            }
            _ => Err(UNSUPPORTED),
        }
    }
    fn policy_request(&mut self, buffer: &mut [u8]) -> Status {
        if &buffer[24..28] != b"VCPC" || u32_at(buffer, 28)? != 1 {
            return Err(INVALID);
        }
        match u32_at(buffer, 32)? {
            1 => Err(PROTECTED),
            2 => {
                *buffer.get_mut(44).ok_or(BAD_SIZE)? = 1;
                Ok(())
            }
            3 => {
                let size = usize::from(u16_at(buffer, 48)?);
                self.register_policy(bounded_range(buffer, 44, size)?)
            }
            4 => {
                bounded_range(buffer, 44, 13)?;
                let page = usize::try_from(u32_at(buffer, 44)?).unwrap();
                let data = self.state.policies.concat();
                let page_size = 16384 - 24 - 20 - 13;
                put32(buffer, 48, data.len());
                if page == 0 {
                    put32(buffer, 52, 0);
                    buffer[56] = u8::from(!data.is_empty());
                    return Err(TOO_SMALL);
                }
                let start = (page - 1).checked_mul(page_size).ok_or(INVALID)?;
                let rest = data.get(start..).ok_or(INVALID)?;
                let size = rest.len().min(page_size);
                bounded_range(buffer, 57, size)?;
                put32(buffer, 52, size);
                buffer[56] = u8::from(rest.len() > size);
                buffer[57..57 + size].copy_from_slice(&rest[..size]);
                Ok(())
            }
            5 => {
                self.state.flags |= 8;
                Ok(())
            }
            6 | 7 => {
                let guid = number(buffer, 44)?;
                let size = usize::try_from(u32_at(buffer, 60)?).unwrap();
                let capacity = usize::try_from(u32_at(buffer, 64)?).unwrap();
                bounded_range(buffer, 68, 44)?;
                let name = bounded_range(buffer, 112, size)?;
                if !valid_name(name, false) {
                    return Err(INVALID);
                }
                let policy = self.matching_policy(guid, name).ok_or(NOT_FOUND)?;
                let (entry, name) = if u32_at(buffer, 32)? == 6 {
                    (&policy.bytes[..44], policy.name)
                } else if policy.bytes[40] == 3 {
                    (
                        &policy.bytes[44..62],
                        &policy.bytes[62..usize::from(u16_at(policy.bytes, 6)?)],
                    )
                } else {
                    return Err(NOT_FOUND);
                };
                put32(buffer, 64, name.len());
                buffer[68..112].fill(0);
                buffer[68..68 + entry.len()].copy_from_slice(entry);
                if capacity < name.len() {
                    return Err(TOO_SMALL);
                }
                bounded_range(buffer, 112, name.len())?;
                buffer[112..112 + name.len()].copy_from_slice(name);
                Ok(())
            }
            _ => Err(UNSUPPORTED),
        }
    }
}
