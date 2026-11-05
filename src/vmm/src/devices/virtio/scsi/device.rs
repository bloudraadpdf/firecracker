// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

use std::io;
use std::path::PathBuf;

use vmm_sys_util::eventfd::EventFd;

use super::ScsiState;
use crate::devices::legacy::atapi::{Atapi, Channel, open_media};
use crate::devices::virtio::device::VirtioDeviceType;
use crate::devices::virtio::queued::Protocol;

const MAX_DATA: usize = 1024 * 1024;
const MAX_HEADER: usize = 4096;

#[derive(Debug)]
pub struct Scsi {
    drives: Vec<(Option<PathBuf>, Atapi)>,
    config: [u8; 36],
}

impl Scsi {
    pub fn new(paths: &[Option<PathBuf>]) -> io::Result<Self> {
        if !(1..=2).contains(&paths.len()) {
            return Err(io::Error::other("invalid SCSI optical topology"));
        }
        let drives = paths
            .iter()
            .map(|path| {
                let mut drive = Atapi::new(
                    open_media(path.as_deref())?,
                    EventFd::new(libc::EFD_NONBLOCK)?,
                    Channel::Primary,
                )?;
                drive.set_legacy_irq_enabled(false);
                Ok((path.clone(), drive))
            })
            .collect::<io::Result<_>>()?;
        let mut device = Self {
            drives,
            config: [0; 36],
        };
        for (index, value) in [
            1u32,
            254,
            u32::try_from(MAX_DATA / 512).unwrap(),
            128,
            16,
            96,
            32,
        ]
        .into_iter()
        .enumerate()
        {
            device.config[index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
        // Advertise address limits, not populated slots: older vioscsi treats these as counts.
        device.config[30..32].copy_from_slice(&255u16.to_le_bytes());
        device.config[32..36].copy_from_slice(&16383u32.to_le_bytes());
        Ok(device)
    }

    pub fn paths(&self) -> impl Iterator<Item = &Option<PathBuf>> {
        self.drives.iter().map(|(path, _)| path)
    }

    pub fn replace_media(&mut self, target: usize, path: Option<PathBuf>) -> io::Result<()> {
        let Some((current, drive)) = self.drives.get_mut(target) else {
            return Err(io::Error::other("unknown SCSI target"));
        };
        drive.replace_media(open_media(path.as_deref())?)?;
        *current = path;
        Ok(())
    }

    pub fn eject(&mut self, target: usize) -> io::Result<()> {
        self.drives
            .get_mut(target)
            .ok_or_else(|| io::Error::other("unknown SCSI target"))?
            .1
            .eject()
    }

    fn field(&self, offset: usize) -> u32 {
        u32::from_le_bytes(self.config[offset..offset + 4].try_into().unwrap())
    }

    fn target(&self, lun: &[u8]) -> Option<usize> {
        (lun.len() == 8
            && lun[0] == 1
            && usize::from(lun[1]) < self.drives.len()
            && matches!(lun[2], 0 | 0x40)
            && lun[3..].iter().all(|&byte| byte == 0))
        .then(|| usize::from(lun[1]))
    }

    fn command(&mut self, request: &[u8], capacity: usize) -> io::Result<Vec<u8>> {
        let sense_size = self.field(20) as usize;
        let cdb_size = self.field(24) as usize;
        let header = 12 + sense_size;
        if sense_size > MAX_HEADER
            || !(12..=MAX_HEADER).contains(&cdb_size)
            || request.len() < 19 + cdb_size
            || capacity < header
            || capacity - header > MAX_DATA
        {
            return Err(io::Error::other("invalid SCSI request layout"));
        }
        let data_capacity = capacity - header;
        let data_out = request.len() - 19 - cdb_size;
        let mut response = vec![0; header];
        response[4..8].copy_from_slice(
            &u32::try_from(data_capacity + data_out)
                .map_err(io::Error::other)?
                .to_le_bytes(),
        );
        let Some(target) = self.target(&request[..8]) else {
            response[11] = 3;
            return Ok(response);
        };
        if data_out != 0 && data_capacity != 0 {
            response[11] = 9;
            return Ok(response);
        }
        match self.drives[target]
            .1
            .scsi_command(&request[19..19 + cdb_size], data_capacity)
        {
            Ok((data, sense)) => {
                response[10] = if sense.is_empty() { 0 } else { 2 };
                let sense_len = sense.len().min(sense_size);
                response[..4].copy_from_slice(&u32::try_from(sense_len).unwrap().to_le_bytes());
                response[12..12 + sense_len].copy_from_slice(&sense[..sense_len]);
                response[4..8].copy_from_slice(
                    &u32::try_from(data_capacity + data_out - data.len())
                        .map_err(io::Error::other)?
                        .to_le_bytes(),
                );
                response.extend_from_slice(&data);
            }
            Err(error) if error.kind() == io::ErrorKind::WriteZero => response[11] = 1,
            Err(error) => return Err(error),
        }
        Ok(response)
    }

    fn control(&mut self, request: &[u8], capacity: usize) -> io::Result<Vec<u8>> {
        if request.len() < 4 {
            return Err(io::Error::other("short SCSI control request"));
        }
        match u32::from_le_bytes(request[..4].try_into().unwrap()) {
            0 if request.len() == 24 && capacity >= 1 => {
                let Some(target) = self.target(&request[8..16]) else {
                    return Ok(vec![3]);
                };
                let subtype = u32::from_le_bytes(request[4..8].try_into().unwrap());
                let response = match subtype {
                    0..=3 | 6..=7 => 0,
                    4 | 5 => {
                        self.drives[target].1.scsi_reset();
                        0
                    }
                    _ => 11,
                };
                Ok(vec![response])
            }
            1 | 2 if request.len() == 16 && capacity >= 5 => Ok(vec![
                0,
                0,
                0,
                0,
                if self.target(&request[4..12]).is_some() {
                    0
                } else {
                    3
                },
            ]),
            _ => Err(io::Error::other("invalid SCSI control request")),
        }
    }
}

impl Protocol for Scsi {
    type State = ScsiState;
    const TYPE: VirtioDeviceType = VirtioDeviceType::Scsi;
    const ID: &'static str = "scsi";
    const FEATURES: u64 = 1 << 32;
    const QUEUES: usize = 3;
    const REQUIRED: &'static [usize] = &[2];
    const REQUEST_LIMIT: usize = MAX_DATA + MAX_HEADER + 19;
    const RESPONSE_LIMIT: usize = MAX_DATA + MAX_HEADER + 12;
    fn config(&self) -> &[u8] {
        &self.config
    }
    fn write_config(&mut self, offset: u64, bytes: &[u8]) {
        if let Ok(offset) = usize::try_from(offset)
            && offset >= 20
            && offset.checked_add(bytes.len()).is_some_and(|end| end <= 28)
        {
            self.config[offset..offset + bytes.len()].copy_from_slice(bytes);
        }
    }
    fn reset(&mut self) {
        self.config[20..24].copy_from_slice(&96u32.to_le_bytes());
        self.config[24..28].copy_from_slice(&32u32.to_le_bytes());
        for (_, drive) in &mut self.drives {
            drive.scsi_reset();
        }
    }
    fn process(
        &mut self,
        queue: usize,
        request: &[u8],
        capacity: usize,
    ) -> io::Result<Option<Vec<u8>>> {
        match queue {
            0 => self.control(request, capacity).map(Some),
            1 if request.is_empty() && capacity >= 16 => Ok(None),
            2 => self.command(request, capacity).map(Some),
            _ => Err(io::Error::other("invalid SCSI queue buffer")),
        }
    }
    fn save(&self) -> Self::State {
        ScsiState {
            drives: self
                .drives
                .iter()
                .map(|(path, drive)| (path.clone(), drive.save()))
                .collect(),
            sense_size: self.field(20),
            cdb_size: self.field(24),
        }
    }
    fn restore(state: &Self::State) -> io::Result<Self> {
        let mut device = Self::new(
            &state
                .drives
                .iter()
                .map(|(path, _)| path.clone())
                .collect::<Vec<_>>(),
        )?;
        for ((_, drive), (path, saved)) in device.drives.iter_mut().zip(&state.drives) {
            saved.validate_scsi()?;
            *drive = Atapi::restore(
                open_media(path.as_deref())?,
                EventFd::new(libc::EFD_NONBLOCK)?,
                saved,
                Channel::Primary,
            )?;
            drive.set_legacy_irq_enabled(false);
        }
        device.config[20..24].copy_from_slice(&state.sense_size.to_le_bytes());
        device.config[24..28].copy_from_slice(&state.cdb_size.to_le_bytes());
        Ok(device)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inquiry() -> Vec<u8> {
        let mut request = vec![0; 51];
        request[0] = 1;
        request[2] = 0x40;
        request[19] = 0x12;
        request[23] = 36;
        request
    }

    #[test]
    fn optical_transport_reports_targets_bounds_and_sense() {
        let mut device = Scsi::new(&[None, None]).unwrap();
        let max_target = u16::from_le_bytes(device.config[30..32].try_into().unwrap());
        let max_lun = device.field(32);
        assert_eq!((max_target, max_lun), (255, 16383));
        for target in 0..2 {
            assert!(target < max_target && max_lun > 0);
        }
        let mut request = inquiry();
        let response = device.command(&request, 144).unwrap();
        assert_eq!(response.len(), 144);
        assert_eq!(&response[10..12], &[0, 0]);
        assert_eq!(response[108], 5);
        request[1] = 2;
        assert_eq!(device.command(&request, 144).unwrap()[11], 3);
        request[1] = 1;
        assert_eq!(device.command(&request, 143).unwrap()[11], 1);
        device.command(&request, 107).unwrap_err();
        request[19] = 0;
        request[23] = 0;
        let response = device.command(&request, 108).unwrap();
        assert_eq!(&response[10..12], &[2, 0]);
        assert_eq!((response[14], response[24]), (2, 0x3a));
        request.push(1);
        assert_eq!(device.command(&request, 109).unwrap()[11], 9);
        device.write_config(20, &u32::MAX.to_le_bytes());
        device.command(&request, 109).unwrap_err();
        device.reset();
        assert_eq!(device.field(20), 96);
        assert_eq!(device.field(24), 32);
        assert_eq!(Scsi::restore(&device.save()).unwrap().save(), device.save());
    }

    #[test]
    fn restored_media_must_match_the_saved_digest() {
        use std::os::unix::fs::FileExt;
        let media = vmm_sys_util::tempfile::TempFile::new().unwrap();
        media.as_file().write_all_at(&[0; 2048], 0).unwrap();
        let device = Scsi::new(&[Some(media.as_path().into())]).unwrap();
        let saved = device.save();
        assert_eq!(Scsi::restore(&saved).unwrap().save(), saved);
        media.as_file().write_all_at(&[1], 0).unwrap();
        Scsi::restore(&saved).unwrap_err();
    }
}
