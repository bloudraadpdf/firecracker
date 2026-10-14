// SPDX-License-Identifier: GPL-2.0-or-later
use crate::engine::{Engine, State};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::OpenOptionsExt,
            net::{UnixListener, UnixStream},
        },
    },
    path::{Path, PathBuf},
};

const MAX_FRAME: usize = 4 * 1024 * 1024;
const MAX_CHECKPOINT: usize = MAX_FRAME - 256;
const GLOBAL_VARIABLE: [u8; 16] = [
    0x61, 0xdf, 0xe4, 0x8b, 0xca, 0x93, 0xd2, 0x11, 0xaa, 0x0d, 0, 0xe0, 0x98, 3, 0x2b, 0x8c,
];
const IMAGE_SECURITY_DATABASE: [u8; 16] = [
    0xcb, 0xb2, 0x19, 0xd7, 0x3a, 0x3d, 0x96, 0x45, 0xa3, 0xbc, 0xda, 0xd0, 0x0e, 0x67, 0x65, 0x6f,
];
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    pub version: u32,
    pub identity: [u8; 16],
    pub generation: u64,
    pub state: State,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Restore {
    expected_generation: u64,
    checkpoint: Checkpoint,
}
fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
fn encode(value: &impl Serialize) -> io::Result<Vec<u8>> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_CHECKPOINT {
        return Err(invalid("state exceeds limit"));
    }
    Ok(bytes)
}
fn read_bounded(path: &Path) -> io::Result<Vec<u8>> {
    let mut result = Vec::new();
    File::open(path)?
        .take(u64::try_from(MAX_FRAME + 1).unwrap())
        .read_to_end(&mut result)?;
    if result.len() > MAX_FRAME {
        return Err(invalid("file exceeds limit"));
    }
    Ok(result)
}
fn directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}
fn persist(path: &Path, checkpoint: &Checkpoint) -> io::Result<()> {
    let bytes = encode(checkpoint)?;
    let pending = path.with_extension("pending");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&pending)?;
    let result = (|| {
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&pending, path)?;
        File::open(directory(path))?.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(pending);
    }
    result
}
struct Store {
    path: PathBuf,
    _lock: File,
    checkpoint: Checkpoint,
    engine: Engine,
}
impl Store {
    fn open(path: &Path) -> io::Result<Self> {
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path.with_extension("lock"))?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let checkpoint: Checkpoint = serde_json::from_slice(&read_bounded(path)?)?;
        if checkpoint.version != 1 {
            return Err(invalid("unsupported checkpoint version"));
        }
        let engine = Engine::restore(&checkpoint.state).map_err(invalid)?;
        Ok(Self {
            path: path.to_owned(),
            _lock: lock,
            checkpoint,
            engine,
        })
    }
    fn commit(&mut self) -> io::Result<()> {
        let state = self.engine.save();
        if state != self.checkpoint.state {
            self.checkpoint.generation = self
                .checkpoint
                .generation
                .checked_add(1)
                .ok_or_else(|| invalid("generation exhausted"))?;
            self.checkpoint.state = state;
            persist(&self.path, &self.checkpoint)?;
        }
        Ok(())
    }
    fn restore(&mut self, mut request: Restore) -> io::Result<()> {
        if request.expected_generation != self.checkpoint.generation
            || request.checkpoint.identity != self.checkpoint.identity
            || request.checkpoint.version != 1
        {
            return Err(invalid("checkpoint identity or generation mismatch"));
        }
        let engine = Engine::restore(&request.checkpoint.state).map_err(invalid)?;
        request.checkpoint.generation = self
            .checkpoint
            .generation
            .checked_add(1)
            .ok_or_else(|| invalid("generation exhausted"))?;
        // Replacing all state is the explicit management rollback boundary.
        persist(&self.path, &request.checkpoint)?;
        self.checkpoint = request.checkpoint;
        self.engine = engine;
        Ok(())
    }
}
fn read_frame(stream: &mut impl Read) -> io::Result<Option<(u16, u32, Vec<u8>)>> {
    let mut header = [0; 16];
    match stream.read(&mut header[..1])? {
        0 => return Ok(None),
        1 => (),
        _ => unreachable!(),
    }
    stream.read_exact(&mut header[1..])?;
    if &header[..4] != b"FCUV" || header[4..6] != 1u16.to_le_bytes() {
        return Err(invalid("invalid frame header"));
    }
    let opcode = u16::from_le_bytes(header[6..8].try_into().unwrap());
    let id = u32::from_le_bytes(header[8..12].try_into().unwrap());
    let size = usize::try_from(u32::from_le_bytes(header[12..16].try_into().unwrap())).unwrap();
    if size > MAX_FRAME || opcode & 0x8000 != 0 {
        return Err(invalid("invalid frame size or opcode"));
    }
    let mut bytes = vec![0; size];
    stream.read_exact(&mut bytes)?;
    Ok(Some((opcode, id, bytes)))
}
fn write_frame(
    stream: &mut impl Write,
    opcode: u16,
    id: u32,
    status: u32,
    bytes: &[u8],
) -> io::Result<()> {
    let mut header = Vec::with_capacity(20);
    header.extend(b"FCUV");
    header.extend(1u16.to_le_bytes());
    header.extend((opcode | 0x8000).to_le_bytes());
    header.extend(id.to_le_bytes());
    header.extend(
        u32::try_from(bytes.len() + 4)
            .map_err(|_| invalid("oversized reply"))?
            .to_le_bytes(),
    );
    header.extend(status.to_le_bytes());
    stream.write_all(&header)?;
    stream.write_all(bytes)
}
fn connection(store: &mut Store, stream: &mut UnixStream) -> io::Result<()> {
    let mut started = false;
    let mut initialised = false;
    while let Some((opcode, id, mut payload)) = read_frame(stream)? {
        let mut reply = Vec::new();
        let mut status = 0;
        match opcode {
            1 if initialised => {
                started = true;
                match store.engine.transact(&mut payload) {
                    Ok(code) => {
                        store.commit()?;
                        reply.extend(code.to_le_bytes());
                        reply.extend(payload);
                    }
                    Err(error) => return Err(invalid(error)),
                }
            }
            2 if payload.is_empty() => reply = encode(&store.checkpoint)?,
            3 if !started => match serde_json::from_slice::<Restore>(&payload) {
                Ok(request) => {
                    store.restore(request)?;
                    initialised = true;
                }
                Err(_) => status = 2,
            },
            4 if !started && !initialised && payload.is_empty() => {
                store.engine.cold_boot();
                store.commit()?;
                initialised = true;
            }
            _ => status = 1,
        }
        write_frame(stream, opcode, id, status, &reply)?;
    }
    Ok(())
}
pub fn serve(path: &Path, socket: &Path) -> io::Result<()> {
    let mut store = Store::open(path)?;
    // This standalone service has one thread; restrict the socket from creation.
    unsafe {
        libc::umask(0o077);
    }
    let listener = UnixListener::bind(socket)?;
    fs::set_permissions(socket, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    let result = listener
        .accept()
        .and_then(|(mut stream, _)| connection(&mut store, &mut stream));
    let _ = fs::remove_file(socket);
    result
}
pub fn provision(path: &Path, seed: &Path) -> io::Result<()> {
    let state: State = serde_json::from_slice(&read_bounded(seed)?)?;
    if state.flags != 0 || !state.policies.is_empty() {
        return Err(invalid("provision requires no lifecycle state"));
    }
    for (name, guid) in [
        ("PK", GLOBAL_VARIABLE),
        ("KEK", GLOBAL_VARIABLE),
        ("db", IMAGE_SECURITY_DATABASE),
    ] {
        let expected: Vec<u8> = name
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect();
        if !state.variables.iter().any(|v| {
            v.name == expected && v.guid == guid && v.attributes == 0x27 && !v.data.is_empty()
        }) {
            return Err(invalid("provision requires PK, KEK and db"));
        }
    }
    let mut engine = Engine::restore(&state).map_err(invalid)?;
    engine.cold_boot();
    let secure_boot_name: Vec<u8> = "SecureBoot"
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect();
    if !engine
        .save()
        .variables
        .iter()
        .any(|v| v.guid == GLOBAL_VARIABLE && v.name == secure_boot_name && v.data == [1])
    {
        return Err(invalid("provisioned state does not enable Secure Boot"));
    }
    let mut identity = [0; 16];
    File::open("/dev/urandom")?.read_exact(&mut identity)?;
    let checkpoint = Checkpoint {
        version: 1,
        identity,
        generation: 0,
        state: engine.save(),
    };
    // Refuse to overwrite an existing store, including a symlink.
    let bytes = encode(&checkpoint)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    if let Err(error) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(path);
        return Err(error);
    }
    File::open(directory(path))?.sync_all()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn relative_store_directory() {
        assert_eq!(directory(Path::new("store.json")), Path::new("."));
        assert_eq!(directory(Path::new("state/store.json")), Path::new("state"));
    }
    #[test]
    fn rejects_truncated_and_oversized_frames() {
        assert!(read_frame(&mut &b"FCUV"[..]).is_err());
        let mut frame = Vec::from(&b"FCUV\x01\x00\x01\x00\x00\x00\x00\x00"[..]);
        frame.extend(u32::MAX.to_le_bytes());
        assert!(read_frame(&mut frame.as_slice()).is_err());
    }
    #[test]
    fn frame_roundtrip_reply_encoding() {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, 2, 41, 0, b"test").unwrap();
        assert_eq!(&bytes[..8], b"FCUV\x01\x00\x02\x80");
        assert_eq!(&bytes[8..12], &41u32.to_le_bytes());
        assert_eq!(&bytes[12..16], &8u32.to_le_bytes());
        assert_eq!(&bytes[20..], b"test");
    }
    #[test]
    fn generation_mismatch_and_staged_rollback_preserve_live_store() {
        let root = std::env::current_dir()
            .unwrap()
            .join("build")
            .join(format!("uefi-vars-state-test-{}", std::process::id()));
        fs::create_dir_all(root.parent().unwrap()).unwrap();
        fs::create_dir(&root).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                fs::remove_dir_all(&self.0).unwrap();
            }
        }
        let _cleanup = Cleanup(root.clone());
        let path = root.join("live.json");
        let mut engine = Engine::new();
        engine.cold_boot();
        let original = Checkpoint {
            version: 1,
            identity: [0x42; 16],
            generation: 7,
            state: engine.save(),
        };
        persist(&path, &original).unwrap();
        let original_bytes = fs::read(&path).unwrap();
        let mut live = Store::open(&path).unwrap();
        assert!(Store::open(&path).is_err());
        let mut older = original.clone();
        older.generation = 3;
        older.state.flags = 3;
        assert!(
            live.restore(Restore {
                expected_generation: 6,
                checkpoint: older.clone()
            })
            .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), original_bytes);
        let staging = root.join("staging.json");
        fs::copy(&path, &staging).unwrap();
        let mut staged = Store::open(&staging).unwrap();
        staged
            .restore(Restore {
                expected_generation: 7,
                checkpoint: older.clone(),
            })
            .unwrap();
        assert_eq!(staged.checkpoint.state, older.state);
        assert_eq!(staged.checkpoint.generation, 8);
        assert!(
            staged
                .restore(Restore {
                    expected_generation: 7,
                    checkpoint: original.clone()
                })
                .is_err()
        );
        drop(staged);
        // A later TPM/disk load fails: discard staging, leaving the live store untouched.
        fs::remove_file(staging).unwrap();
        assert_eq!(fs::read(&path).unwrap(), original_bytes);
        assert_eq!(live.checkpoint, original);
    }
}
