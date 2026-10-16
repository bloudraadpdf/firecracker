// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
use std::fmt::Debug;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use vmm::seccomp::{BpfThreadMap, DeserializationError, deserialize_binary, get_empty_filters};

const THREAD_CATEGORIES: [&str; 3] = ["vmm", "api", "vcpu"];

/// Error retrieving seccomp filters.
#[derive(Debug, thiserror::Error, displaydoc::Display)]
pub enum FilterError {
    /// Filter deserialization failed: {0}
    Deserialization(DeserializationError),
    /// Invalid thread categories: {0}
    ThreadCategories(String),
    /// Missing thread category: {0}
    MissingThreadCategory(String),
    /// Filter file open error: {0}
    FileOpen(std::io::Error),
}

/// Seccomp filter configuration.
#[derive(Debug)]
pub enum SeccompConfig {
    /// Seccomp filtering disabled.
    None,
    /// Default, advanced filters.
    Advanced,
    /// Custom, user-provided filters.
    Custom(File),
}

impl SeccompConfig {
    /// Given the relevant command line args, return the appropriate config type.
    pub fn from_args<T: AsRef<Path> + Debug>(
        no_seccomp: bool,
        seccomp_filter: Option<T>,
    ) -> Result<Self, FilterError> {
        if no_seccomp {
            Ok(SeccompConfig::None)
        } else {
            match seccomp_filter {
                Some(path) => Ok(SeccompConfig::Custom(
                    File::open(path).map_err(FilterError::FileOpen)?,
                )),
                None => Ok(SeccompConfig::Advanced),
            }
        }
    }
}

/// Retrieve the appropriate filters, based on the SeccompConfig.
pub fn get_filters(config: SeccompConfig) -> Result<BpfThreadMap, FilterError> {
    match config {
        SeccompConfig::None => Ok(get_empty_filters()),
        SeccompConfig::Advanced => get_default_filters(),
        SeccompConfig::Custom(reader) => get_custom_filters(reader),
    }
}

/// Retrieve the default filters containing the syscall rules required by `Firecracker`
/// to function. The binary file is generated via the `build.rs` script of this crate.
fn get_default_filters() -> Result<BpfThreadMap, FilterError> {
    // Retrieve, at compile-time, the serialized binary filter generated with seccompiler.
    let bytes: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/seccomp_filter.bpf"));
    let map = deserialize_binary(bytes).map_err(FilterError::Deserialization)?;
    filter_thread_categories(map)
}

/// Retrieve custom seccomp filters.
fn get_custom_filters<R: Read + Debug>(reader: R) -> Result<BpfThreadMap, FilterError> {
    let map = deserialize_binary(BufReader::new(reader)).map_err(FilterError::Deserialization)?;
    filter_thread_categories(map)
}

/// Return an error if the BpfThreadMap contains invalid thread categories.
fn filter_thread_categories(map: BpfThreadMap) -> Result<BpfThreadMap, FilterError> {
    let (filters, invalid_filters): (BpfThreadMap, BpfThreadMap) = map
        .into_iter()
        .partition(|(k, _)| THREAD_CATEGORIES.contains(&k.as_str()));
    if !invalid_filters.is_empty() {
        // build the error message
        let mut thread_categories_string =
            invalid_filters
                .keys()
                .fold("".to_string(), |mut acc, elem| {
                    acc.push_str(elem);
                    acc.push(',');
                    acc
                });
        thread_categories_string.pop();
        return Err(FilterError::ThreadCategories(thread_categories_string));
    }

    for &category in THREAD_CATEGORIES.iter() {
        let category_string = category.to_string();
        if !filters.contains_key(&category_string) {
            return Err(FilterError::MissingThreadCategory(category_string));
        }
    }

    Ok(filters)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use vmm::seccomp::BpfThreadMap;
    use vmm_sys_util::tempfile::TempFile;

    use super::*;

    #[cfg(all(target_env = "musl", not(debug_assertions)))]
    fn apply_vcpu_filter() {
        let filters = get_default_filters().unwrap();
        let filter = &filters["vcpu"];
        assert!(!filter.is_empty());
        let limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: limit points to a valid rlimit structure.
        assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_CORE, &limit) }, 0);
        vmm::seccomp::apply_filter(filter).unwrap();
    }

    #[cfg(all(target_env = "musl", not(debug_assertions)))]
    #[test]
    fn vcpu_filter_allows_tap_header_negotiation() {
        use std::os::fd::AsRawFd;
        use std::os::unix::process::ExitStatusExt;
        use std::process::Command;
        use vmm::devices::virtio::net::Tap;

        const PROBE: &str = "FIRECRACKER_TEST_TAP_HEADER";
        if let Ok(value) = std::env::var(PROBE) {
            let size: i32 = value.parse().unwrap();
            let tap = Tap::open_named("fcsec%d").unwrap();
            apply_vcpu_filter();
            // SAFETY: tap is open and size is a valid integer ioctl argument.
            unsafe {
                if size == 0 {
                    libc::ioctl(tap.as_raw_fd(), 0x400454dc, &size);
                    libc::_exit(1);
                }
                libc::_exit(i32::from(tap.set_vnet_hdr_size(size).is_err()));
            }
        }
        for size in [10, 12, 0] {
            let status = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "seccomp::tests::vcpu_filter_allows_tap_header_negotiation",
                ])
                .env(PROBE, size.to_string())
                .status()
                .unwrap();
            if size == 0 {
                assert_eq!(status.signal(), Some(libc::SIGSYS));
            } else {
                assert!(status.success(), "TAP header {size}: {status:?}");
            }
        }
    }

    #[test]
    fn test_get_filters() {
        let mut filters = get_empty_filters();
        assert_eq!(filters.len(), 3);
        assert!(filters.remove("vmm").is_some());
        assert!(filters.remove("api").is_some());
        assert!(filters.remove("vcpu").is_some());

        let mut filters = get_empty_filters();
        assert_eq!(filters.len(), 3);
        assert_eq!(filters.remove("vmm").unwrap().len(), 0);
        assert_eq!(filters.remove("api").unwrap().len(), 0);
        assert_eq!(filters.remove("vcpu").unwrap().len(), 0);

        let file = TempFile::new().unwrap().into_file();

        get_filters(SeccompConfig::Custom(file)).unwrap_err();
    }

    #[test]
    fn test_filter_thread_categories() {
        // correct categories
        let mut map = BpfThreadMap::new();
        map.insert("vcpu".to_string(), Arc::new(vec![]));
        map.insert("vmm".to_string(), Arc::new(vec![]));
        map.insert("api".to_string(), Arc::new(vec![]));

        assert_eq!(filter_thread_categories(map).unwrap().len(), 3);

        // invalid categories
        let mut map = BpfThreadMap::new();
        map.insert("vcpu".to_string(), Arc::new(vec![]));
        map.insert("vmm".to_string(), Arc::new(vec![]));
        map.insert("thread1".to_string(), Arc::new(vec![]));
        map.insert("thread2".to_string(), Arc::new(vec![]));

        match filter_thread_categories(map).unwrap_err() {
            FilterError::ThreadCategories(err) => {
                assert!(err == "thread2,thread1" || err == "thread1,thread2")
            }
            _ => panic!("Expected ThreadCategories error."),
        }

        // missing category
        let mut map = BpfThreadMap::new();
        map.insert("vcpu".to_string(), Arc::new(vec![]));
        map.insert("vmm".to_string(), Arc::new(vec![]));

        match filter_thread_categories(map).unwrap_err() {
            FilterError::MissingThreadCategory(name) => assert_eq!(name, "api"),
            _ => panic!("Expected MissingThreadCategory error."),
        }
    }

    #[test]
    fn test_seccomp_config() {
        assert!(matches!(
            SeccompConfig::from_args(true, Option::<&str>::None),
            Ok(SeccompConfig::None)
        ));

        assert!(matches!(
            SeccompConfig::from_args(false, Some("/dev/null")),
            Ok(SeccompConfig::Custom(_))
        ));

        assert!(matches!(
            SeccompConfig::from_args(false, Some("invalid_path")),
            Err(FilterError::FileOpen(_))
        ));

        // test the default case, no parametes -> default advanced.
        assert!(matches!(
            SeccompConfig::from_args(false, Option::<&str>::None),
            Ok(SeccompConfig::Advanced)
        ));
    }
}
