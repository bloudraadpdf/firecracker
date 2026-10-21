// Copyright 2020 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Defines the structures needed for saving/restoring block devices.

use device::ConfigSpace;
use serde::{Deserialize, Serialize};
use vmm_sys_util::eventfd::EventFd;

use super::device::DiskProperties;
use super::*;
use crate::devices::virtio::block::persist::BlockConstructorArgs;
use crate::devices::virtio::block::virtio::device::FileEngineType;
use crate::devices::virtio::block::virtio::device::VirtioBlkTopology;
use crate::devices::virtio::block::virtio::metrics::BlockMetricsPerDevice;
use crate::devices::virtio::device::{DeviceState, VirtioDeviceType};
use crate::devices::virtio::generated::virtio_blk::VIRTIO_BLK_F_RO;
use crate::devices::virtio::persist::VirtioDeviceState;
use crate::rate_limiter::RateLimiter;
use crate::rate_limiter::persist::RateLimiterState;
use crate::snapshot::Persist;

/// Holds info about block's file engine type. Gets saved in snapshot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileEngineTypeState {
    /// Sync File Engine.
    // If the snap version does not contain the `FileEngineType`, it must have been snapshotted
    // on a VM using the Sync backend.
    #[default]
    Sync,
    /// Async File Engine.
    Async,
}

impl From<FileEngineType> for FileEngineTypeState {
    fn from(file_engine_type: FileEngineType) -> Self {
        match file_engine_type {
            FileEngineType::Sync => FileEngineTypeState::Sync,
            FileEngineType::Async => FileEngineTypeState::Async,
        }
    }
}

impl From<FileEngineTypeState> for FileEngineType {
    fn from(file_engine_type_state: FileEngineTypeState) -> Self {
        match file_engine_type_state {
            FileEngineTypeState::Sync => FileEngineType::Sync,
            FileEngineTypeState::Async => FileEngineType::Async,
        }
    }
}

/// Holds info about the block device. Gets saved in snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtioBlockState {
    id: String,
    partuuid: Option<String>,
    cache_type: CacheType,
    root_device: bool,
    disk_path: String,
    inherited: bool,
    pub virtio_state: VirtioDeviceState,
    rate_limiter_state: RateLimiterState,
    file_engine_type: FileEngineTypeState,
    blk_size: u32,
    topology: VirtioBlkTopology,
}

impl Persist<'_> for VirtioBlock {
    type State = VirtioBlockState;
    type ConstructorArgs = BlockConstructorArgs;
    type Error = VirtioBlockError;

    fn save(&self) -> Self::State {
        // Save device state.
        VirtioBlockState {
            id: self.id.clone(),
            partuuid: self.partuuid.clone(),
            cache_type: self.cache_type,
            root_device: self.root_device,
            disk_path: self.disk.file_path.clone(),
            inherited: self.disk.inherited,
            virtio_state: VirtioDeviceState::from_device(self),
            rate_limiter_state: self.rate_limiter.save(),
            file_engine_type: FileEngineTypeState::from(self.file_engine_type()),
            blk_size: self.config_space.blk_size,
            topology: self.config_space.topology,
        }
    }

    fn restore(
        constructor_args: Self::ConstructorArgs,
        state: &Self::State,
    ) -> Result<Self, Self::Error> {
        if state.inherited != constructor_args.backing.is_some() {
            return Err(VirtioBlockError::Config);
        }
        let is_read_only = state.virtio_state.avail_features & (1u64 << VIRTIO_BLK_F_RO) != 0;
        let rate_limiter = RateLimiter::restore((), &state.rate_limiter_state)
            .map_err(VirtioBlockError::RateLimiter)?;

        let disk_properties = DiskProperties::new(
            state.disk_path.clone(),
            is_read_only,
            state.file_engine_type.into(),
            constructor_args.backing,
        )?;

        let queue_evts = [EventFd::new(libc::EFD_NONBLOCK).map_err(VirtioBlockError::EventFd)?];

        let queues = state
            .virtio_state
            .build_queues_checked(
                &constructor_args.mem,
                VirtioDeviceType::Block,
                BLOCK_NUM_QUEUES,
                FIRECRACKER_MAX_QUEUE_SIZE,
            )
            .map_err(VirtioBlockError::Persist)?;

        let avail_features = state.virtio_state.avail_features;
        let acked_features = state.virtio_state.acked_features;

        let config_space = ConfigSpace {
            capacity: disk_properties.nsectors.to_le(),
            blk_size: state.blk_size,
            topology: state.topology,
            ..Default::default()
        };

        Ok(VirtioBlock {
            avail_features,
            acked_features,
            config_space,
            activate_evt: EventFd::new(libc::EFD_NONBLOCK).map_err(VirtioBlockError::EventFd)?,

            queues,
            queue_evts,
            device_state: DeviceState::Inactive,

            id: state.id.clone(),
            partuuid: state.partuuid.clone(),
            cache_type: state.cache_type,
            root_device: state.root_device,
            read_only: is_read_only,

            disk: disk_properties,
            rate_limiter,
            is_io_engine_throttled: false,
            metrics: BlockMetricsPerDevice::alloc(state.id.clone()),
        })
    }
}

#[cfg(test)]
mod tests {
    use vmm_sys_util::tempfile::TempFile;

    use super::*;
    use crate::devices::virtio::block::virtio::device::VirtioBlockConfig;
    use crate::devices::virtio::device::VirtioDevice;
    use crate::devices::virtio::test_utils::default_mem;

    #[test]
    fn inherited_backing_survives_unlink_and_snapshot_without_path_fallback() {
        use std::os::unix::fs::FileExt;
        let fixture = TempFile::new().unwrap();
        fixture.as_file().set_len(4096).unwrap();
        fixture
            .as_file()
            .write_all_at(b"retained-volume", 0)
            .unwrap();
        let source = fixture.as_file().try_clone().unwrap();
        let path = fixture.as_path().to_str().unwrap().to_owned();
        drop(fixture);
        assert!(!std::path::Path::new(&path).exists());
        let config = VirtioBlockConfig {
            drive_id: "volume".to_owned(),
            path_on_host: path.clone(),
            is_root_device: false,
            partuuid: None,
            is_read_only: false,
            cache_type: CacheType::Writeback,
            rate_limiter: None,
            file_engine_type: FileEngineType::Sync,
            blk_size: None,
            topology: None,
        };
        let mut block = VirtioBlock::new(config, Some(source.try_clone().unwrap())).unwrap();
        assert_eq!(block.disk.nsectors, 8);
        assert!(block.update_disk_image(path.clone()).is_err());
        let state = block.save();
        drop(block);
        let bytes = bitcode::serialize(&state).unwrap();
        let state = bitcode::deserialize(&bytes).unwrap();
        assert!(matches!(
            VirtioBlock::restore(
                BlockConstructorArgs {
                    mem: default_mem(),
                    backing: None,
                },
                &state
            ),
            Err(VirtioBlockError::Config)
        ));
        let restored = VirtioBlock::restore(
            BlockConstructorArgs {
                mem: default_mem(),
                backing: Some(source.try_clone().unwrap()),
            },
            &state,
        )
        .unwrap();
        let mut observed = [0; 15];
        restored
            .disk
            .file_engine
            .file()
            .read_exact_at(&mut observed, 0)
            .unwrap();
        assert_eq!(&observed, b"retained-volume");
        assert!(restored.disk.inherited);
        assert_eq!(restored.disk.file_path, path);
    }

    #[test]
    fn test_cache_semantic_ser() {
        // We create the backing file here so that it exists for the whole lifetime of the test.
        let f = TempFile::new().unwrap();
        f.as_file().set_len(0x1000).unwrap();

        let config = VirtioBlockConfig {
            drive_id: "test".to_string(),
            path_on_host: f.as_path().to_str().unwrap().to_string(),
            is_root_device: false,
            partuuid: None,
            is_read_only: false,
            cache_type: CacheType::Writeback,
            rate_limiter: None,
            file_engine_type: FileEngineType::default(),
            blk_size: None,
            topology: None,
        };

        let block = VirtioBlock::new(config, None).unwrap();

        // Save the block device.
        let block_state = block.save();
        let _serialized_data = bitcode::serialize(&block_state).unwrap();
    }

    #[test]
    fn test_file_engine_type() {
        // Test conversions between FileEngineType and FileEngineTypeState.
        assert_eq!(
            FileEngineTypeState::Async,
            FileEngineTypeState::from(FileEngineType::Async)
        );
        assert_eq!(
            FileEngineTypeState::Sync,
            FileEngineTypeState::from(FileEngineType::Sync)
        );
        assert_eq!(FileEngineType::Async, FileEngineTypeState::Async.into());
        assert_eq!(FileEngineType::Sync, FileEngineTypeState::Sync.into());
        // Test default impl.
        assert_eq!(FileEngineTypeState::default(), FileEngineTypeState::Sync);
    }

    #[test]
    fn test_persistence() {
        // We create the backing file here so that it exists for the whole lifetime of the test.
        let f = TempFile::new().unwrap();
        f.as_file().set_len(0x1000).unwrap();

        let config = VirtioBlockConfig {
            drive_id: "test".to_string(),
            path_on_host: f.as_path().to_str().unwrap().to_string(),
            is_root_device: false,
            partuuid: None,
            is_read_only: false,
            cache_type: CacheType::Unsafe,
            rate_limiter: None,
            file_engine_type: FileEngineType::default(),
            blk_size: None,
            topology: None,
        };

        let block = VirtioBlock::new(config, None).unwrap();
        let guest_mem = default_mem();

        // Save the block device.
        let block_state = block.save();
        let serialized_data = bitcode::serialize(&block_state).unwrap();

        // Restore the block device.
        let restored_state = bitcode::deserialize(&serialized_data).unwrap();
        let restored_block = VirtioBlock::restore(
            BlockConstructorArgs {
                mem: guest_mem,
                backing: None,
            },
            &restored_state,
        )
        .unwrap();

        // Test that virtio specific fields are the same.
        assert_eq!(restored_block.device_type(), VirtioDeviceType::Block);
        assert_eq!(restored_block.avail_features(), block.avail_features());
        assert_eq!(restored_block.acked_features(), block.acked_features());
        assert_eq!(restored_block.queues(), block.queues());
        assert!(!block.is_activated());
        assert!(!restored_block.is_activated());

        // Test that block specific fields are the same.
        assert_eq!(restored_block.disk.file_path, block.disk.file_path);
    }
}
