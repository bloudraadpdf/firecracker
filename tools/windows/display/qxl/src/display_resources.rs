// Copyright 2009 Red Hat, Inc.
// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Shared display resources and QXL output/release ownership from display/res.c.

use core::mem::{ManuallyDrop, align_of, size_of};
use core::ptr::{NonNull, read, read_volatile, write, write_bytes};

use crate::arena::{Allocation, Arena};
use crate::ring::{CommandRing, ReleaseRing, RingError};
use crate::slots::{MemorySlot, SlotError};

const MAX_RESOURCES: usize = 6;
const RESOURCE_BYTES: usize = size_of::<ResourceNode<'static>>().next_multiple_of(8);
const OUTPUT_BYTES: usize = size_of::<OutputNode<'static>>().next_multiple_of(8);

const _: () = {
    assert!(align_of::<ResourceNode<'static>>() <= 8);
    assert!(align_of::<OutputNode<'static>>() <= 8);
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidMapping,
    InvalidSize,
    OutOfMemory,
    WrongPool,
    TooManyResources,
    ReferenceLimit,
    RingFull,
    Ring(RingError),
    Address(SlotError),
    UnknownRelease,
    InvalidChain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    Main,
    Video,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Command {
    Draw = 1,
    Update = 2,
    Cursor = 3,
    Message = 4,
    Surface = 5,
}

#[derive(Debug)]
struct ResourceNode<'memory> {
    allocation: ManuallyDrop<Allocation<'memory>>,
    video: Option<Allocation<'memory>>,
    references: u32,
}

/// One resource reference. Transfer it to an output, release it explicitly, or
/// retain another reference for a cache. Dropping the token does not free it.
#[derive(Debug)]
#[must_use]
pub struct Resource<'memory> {
    node: NonNull<ResourceNode<'memory>>,
    owner: NonNull<u8>,
    data: NonNull<u8>,
    length: usize,
}

impl Resource<'_> {
    pub fn as_ptr(&self) -> *mut u8 {
        self.data.as_ptr()
    }

    pub fn len(&self) -> usize {
        self.length
    }

    pub fn is_empty(&self) -> bool {
        false
    }
}

#[derive(Debug)]
struct OutputNode<'memory> {
    allocation: ManuallyDrop<Allocation<'memory>>,
    resources: [Option<Resource<'memory>>; MAX_RESOURCES],
    previous: Option<NonNull<Self>>,
    next: Option<NonNull<Self>>,
    id: u64,
    command: Command,
    length: usize,
}

/// A not-yet-published output. A failed submission returns this token for retry
/// or cancellation; successful publication transfers it to the release queue.
#[derive(Debug)]
#[must_use]
pub struct Output<'memory> {
    node: NonNull<OutputNode<'memory>>,
    owner: NonNull<u8>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reclaimed {
    pub outputs: usize,
    pub notify: bool,
}

#[derive(Debug)]
pub struct Pool<'memory> {
    main: Arena<'memory>,
    video: Option<Arena<'memory>>,
    owner: NonNull<u8>,
    slot: MemorySlot,
    pending: Option<NonNull<OutputNode<'memory>>>,
    next_id: u64,
    release_chain: u64,
    outputs: usize,
    resources: usize,
}

impl<'memory> Pool<'memory> {
    /// # Safety
    /// Supply disjoint, exclusively managed BAR ranges that remain live for
    /// `'memory`, including all outstanding tokens. The slot must describe the
    /// main range in the active QXL mapping. Serialise every pool operation with
    /// the display allocation semaphore. Host/guest access must stay inside
    /// live payloads: allocator and node headers are private. Release all tokens
    /// and completed outputs before resetting or unmapping the device.
    pub unsafe fn new(
        main: NonNull<u8>,
        length: usize,
        video: Option<(NonNull<u8>, usize)>,
        slot: MemorySlot,
    ) -> Result<Self, Error> {
        let end = main
            .as_ptr()
            .addr()
            .checked_add(length)
            .ok_or(Error::InvalidMapping)?;
        if let Some((base, bytes)) = video {
            let video_end = base
                .as_ptr()
                .addr()
                .checked_add(bytes)
                .ok_or(Error::InvalidMapping)?;
            if main.as_ptr().addr() < video_end && base.as_ptr().addr() < end {
                return Err(Error::InvalidMapping);
            }
        }
        slot.encode(main.as_ptr().addr() as u64, length as u64)
            .map_err(Error::Address)?;
        // SAFETY: the caller supplies the disjoint live ranges and their ownership.
        let (main_arena, video) = unsafe {
            let main = Arena::new(main, length).ok_or(Error::InvalidMapping)?;
            let video = match video {
                Some((base, length)) => {
                    Some(Arena::new(base, length).ok_or(Error::InvalidMapping)?)
                }
                None => None,
            };
            (main, video)
        };
        Ok(Self {
            main: main_arena,
            video,
            owner: main,
            slot,
            pending: None,
            next_id: 1,
            release_chain: 0,
            outputs: 0,
            resources: 0,
        })
    }

    pub fn resource(&mut self, region: Region, length: usize) -> Result<Resource<'memory>, Error> {
        if length == 0 {
            return Err(Error::InvalidSize);
        }
        let bytes = RESOURCE_BYTES
            .checked_add(if region == Region::Main { length } else { 0 })
            .ok_or(Error::InvalidSize)?;
        let allocation = self.main.allocate(bytes).ok_or(Error::OutOfMemory)?;
        let (video, data) = if region == Region::Video {
            let Some(video) = self.video.as_mut().and_then(|arena| arena.allocate(length)) else {
                self.free_main(allocation);
                return Err(Error::OutOfMemory);
            };
            let data = video.as_ptr();
            (Some(video), data)
        } else {
            // SAFETY: the allocation contains its header followed by length bytes.
            (None, unsafe { allocation.as_ptr().add(RESOURCE_BYTES) })
        };
        let node = allocation.as_ptr().cast::<ResourceNode<'memory>>();
        // SAFETY: both ranges are disjoint, owned and correctly aligned. Every
        // header field and payload byte is initialised before publication.
        unsafe {
            write_bytes(data, 0, length);
            write(
                node,
                ResourceNode {
                    allocation: ManuallyDrop::new(allocation),
                    video,
                    references: 1,
                },
            );
        }
        self.resources += 1;
        Ok(Resource {
            // SAFETY: successful arena allocations are non-null.
            node: unsafe { NonNull::new_unchecked(node) },
            owner: self.owner,
            // SAFETY: payload addresses lie within successful allocations.
            data: unsafe { NonNull::new_unchecked(data) },
            length,
        })
    }

    pub fn retain(&mut self, resource: &Resource<'memory>) -> Result<Resource<'memory>, Error> {
        if resource.owner != self.owner {
            return Err(Error::WrongPool);
        }
        // SAFETY: the noncopyable token owns a live reference to this pool's
        // private node; all reference-count changes require this pool borrow.
        let node = unsafe { &mut *resource.node.as_ptr() };
        node.references = node
            .references
            .checked_add(1)
            .ok_or(Error::ReferenceLimit)?;
        Ok(Resource {
            node: resource.node,
            owner: resource.owner,
            data: resource.data,
            length: resource.length,
        })
    }

    pub fn release(
        &mut self,
        resource: Resource<'memory>,
    ) -> Result<(), (Error, Resource<'memory>)> {
        if resource.owner != self.owner {
            return Err((Error::WrongPool, resource));
        }
        // SAFETY: this token owns a live reference; node headers cannot alias
        // payload users. The pool borrow serialises count updates.
        let node = unsafe { &mut *resource.node.as_ptr() };
        node.references -= 1;
        if node.references != 0 {
            return Ok(());
        }
        // SAFETY: no other references remain; move ownership off the allocation
        // before freeing either its video payload or its main-memory header.
        let node = unsafe { read(resource.node.as_ptr()) };
        if let Some(video) = node.video {
            self.video
                .as_mut()
                .expect("video resource arena")
                .release(video)
                .expect("video resource ownership");
        }
        self.free_main(ManuallyDrop::into_inner(node.allocation));
        self.resources -= 1;
        Ok(())
    }

    pub fn output(&mut self, command: Command, length: usize) -> Result<Output<'memory>, Error> {
        if length < 8 {
            return Err(Error::InvalidSize);
        }
        let next_id = self.next_id.checked_add(1).ok_or(Error::ReferenceLimit)?;
        let bytes = OUTPUT_BYTES.checked_add(length).ok_or(Error::InvalidSize)?;
        let allocation = self.main.allocate(bytes).ok_or(Error::OutOfMemory)?;
        let node = allocation.as_ptr().cast::<OutputNode<'memory>>();
        // SAFETY: the complete header and trailing payload are owned, aligned
        // and disjoint. No host or guest user has received this output yet.
        unsafe {
            write_bytes(allocation.as_ptr().add(OUTPUT_BYTES), 0, length);
            write(
                node,
                OutputNode {
                    allocation: ManuallyDrop::new(allocation),
                    resources: core::array::from_fn(|_| None),
                    previous: None,
                    next: None,
                    id: self.next_id,
                    command,
                    length,
                },
            );
        }
        self.next_id = next_id;
        self.outputs += 1;
        Ok(Output {
            // SAFETY: arena allocation succeeded.
            node: unsafe { NonNull::new_unchecked(node) },
            owner: self.owner,
        })
    }

    pub fn payload<'a>(
        &'a mut self,
        output: &'a mut Output<'memory>,
    ) -> Result<&'a mut [u8], Error> {
        if output.owner != self.owner {
            return Err(Error::WrongPool);
        }
        // SAFETY: the unique builder token cannot be pending. Its private node
        // retains the complete payload; the result borrows both pool and token.
        let node = unsafe { output.node.as_ref() };
        // SAFETY: only this builder can access its unpublished writable payload.
        Ok(unsafe {
            core::slice::from_raw_parts_mut(node.allocation.as_ptr().add(OUTPUT_BYTES), node.length)
        })
    }

    pub fn attach(
        &mut self,
        output: &mut Output<'memory>,
        resource: Resource<'memory>,
    ) -> Result<(), (Error, Resource<'memory>)> {
        if output.owner != self.owner || resource.owner != self.owner {
            return Err((Error::WrongPool, resource));
        }
        // SAFETY: the builder token exclusively owns this unpublished node.
        let node = unsafe { output.node.as_mut() };
        let Some(entry) = node.resources.iter_mut().find(|entry| entry.is_none()) else {
            return Err((Error::TooManyResources, resource));
        };
        *entry = Some(resource);
        Ok(())
    }

    pub fn cancel(&mut self, output: Output<'memory>) -> Result<(), (Error, Output<'memory>)> {
        if output.owner != self.owner {
            return Err((Error::WrongPool, output));
        }
        self.destroy_output(output.node);
        Ok(())
    }

    /// # Safety
    /// The ring must belong to this device and mapping generation. All payload
    /// and resource writes must have ended before publication, and mappings and
    /// allocations must remain live until the host's release is consumed.
    pub unsafe fn submit(
        &mut self,
        output: Output<'memory>,
        ring: &mut CommandRing,
    ) -> Result<bool, (Error, Output<'memory>)> {
        if output.owner != self.owner {
            return Err((Error::WrongPool, output));
        }
        // SAFETY: the builder owns an unpublished, live private node.
        let node = unsafe { output.node.as_ref() };
        // SAFETY: the complete trailing payload belongs to this node.
        let payload = unsafe { node.allocation.as_ptr().add(OUTPUT_BYTES) };
        let physical = match self.slot.encode(payload.addr() as u64, node.length as u64) {
            Ok(address) => address,
            Err(error) => return Err((Error::Address(error), output)),
        };
        let mut command = [0; 16];
        command[..8].copy_from_slice(&physical.to_le_bytes());
        command[8..12].copy_from_slice(&(node.command as u32).to_le_bytes());
        // SAFETY: the builder owns the first eight payload bytes, including
        // release_info. Restore the opaque ID even if a serializer cleared it.
        unsafe { core::ptr::copy_nonoverlapping(node.id.to_le_bytes().as_ptr(), payload, 8) };
        self.link(output.node);
        match ring.push(&command) {
            Ok(Some(notify)) => Ok(notify),
            result => {
                self.unlink(output.node);
                Err((
                    match result {
                        Ok(None) => Error::RingFull,
                        Err(error) => Error::Ring(error),
                        _ => unreachable!(),
                    },
                    output,
                ))
            }
        }
    }

    /// # Safety
    /// The release ring must belong to the same active device and mapping.
    /// Its host-published chains must report completed commands: after release,
    /// the host must no longer access any payload or its attached resources.
    pub unsafe fn reap(
        &mut self,
        ring: &mut ReleaseRing,
        budget: usize,
    ) -> Result<Reclaimed, Error> {
        let mut result = Reclaimed::default();
        while result.outputs < budget {
            if self.release_chain == 0 {
                let Some((id, notify)) = ring.pop().map_err(Error::Ring)? else {
                    break;
                };
                if id == 0 {
                    return Err(Error::InvalidChain);
                }
                self.release_chain = id;
                result.notify |= notify;
            }
            let node = self.find(self.release_chain).ok_or(Error::UnknownRelease)?;
            // SAFETY: find resolves only a live registered output. The host's
            // completion publication precedes these packed release-info reads.
            let payload = unsafe { node.as_ref().allocation.as_ptr().add(OUTPUT_BYTES) };
            let mut bytes = [0; 8];
            for (index, byte) in bytes.iter_mut().enumerate() {
                // SAFETY: release_info is the first eight bytes of every output.
                *byte = unsafe { read_volatile(payload.add(index)) };
            }
            let next = u64::from_le_bytes(bytes);
            if next == self.release_chain || (next != 0 && self.find(next).is_none()) {
                return Err(Error::InvalidChain);
            }
            self.unlink(node);
            self.destroy_output(node);
            self.release_chain = next;
            result.outputs += 1;
        }
        Ok(result)
    }

    /// # Safety
    /// The device/host must have stopped all command and resource access,
    /// including pending release writes. A timeout alone does not prove this.
    /// Keep BAR mappings live until this call and all token releases finish.
    pub unsafe fn discard_pending(&mut self) {
        self.release_chain = 0;
        while let Some(node) = self.pending {
            self.unlink(node);
            self.destroy_output(node);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.outputs == 0 && self.resources == 0
    }

    fn free_main(&mut self, allocation: Allocation<'memory>) {
        self.main
            .release(allocation)
            .expect("main resource ownership");
    }

    fn destroy_output(&mut self, node: NonNull<OutputNode<'memory>>) {
        // SAFETY: callers hold the builder token, or unlinked the completed
        // output. Move its tokens before releasing the allocation containing it.
        let node = unsafe { read(node.as_ptr()) };
        for resource in node.resources.into_iter().flatten() {
            self.release(resource).expect("output resource ownership");
        }
        self.free_main(ManuallyDrop::into_inner(node.allocation));
        self.outputs -= 1;
    }

    fn link(&mut self, mut node: NonNull<OutputNode<'memory>>) {
        // SAFETY: the unique builder is not linked; pending nodes remain live.
        unsafe {
            node.as_mut().next = self.pending;
            node.as_mut().previous = None;
            if let Some(mut head) = self.pending {
                head.as_mut().previous = Some(node);
            }
        }
        self.pending = Some(node);
    }

    fn unlink(&mut self, mut node: NonNull<OutputNode<'memory>>) {
        // SAFETY: callers pass only a registered pending node. Links are private
        // and updated under the pool borrow before any node allocation is freed.
        unsafe {
            let previous = node.as_ref().previous;
            let next = node.as_ref().next;
            if let Some(mut previous) = previous {
                previous.as_mut().next = next;
            } else {
                self.pending = next;
            }
            if let Some(mut next) = next {
                next.as_mut().previous = previous;
            }
            node.as_mut().previous = None;
            node.as_mut().next = None;
        }
    }

    fn find(&self, id: u64) -> Option<NonNull<OutputNode<'memory>>> {
        let mut current = self.pending;
        while let Some(node) = current {
            // SAFETY: only pool-owned live nodes enter this private list. IDs
            // supplied by the host are compared, never treated as pointers.
            let entry = unsafe { node.as_ref() };
            if entry.id == id {
                return Some(node);
            }
            current = entry.next;
        }
        None
    }
}

#[cfg(test)]
pub(crate) mod test_utils;

#[cfg(test)]
mod tests {
    use super::*;

    use super::test_utils::*;

    #[test]
    fn shared_video_resource_survives_chained_and_budgeted_releases() {
        let mut main = [0u64; 2048];
        let mut video = [0u64; 128];
        let mut pool = pool(&mut main, Some(&mut video));
        let mut commands = Bytes([0; 532]);
        let command_host = HostRing::new(&mut commands, 32, 16);
        command_host.set(8, 1);
        let mut releases = Bytes([0; 84]);
        let mut release_host = HostRing::new(&mut releases, 8, 8);
        release_host.set(16, 1);
        // SAFETY: each live mapping has exactly one guest endpoint.
        let (mut commands, mut releases) = unsafe {
            (
                CommandRing::new(command_host.base, 532).unwrap(),
                ReleaseRing::new(release_host.base, 84).unwrap(),
            )
        };
        let cached = pool.resource(Region::Video, 256).unwrap();
        // SAFETY: this unique new resource has not been submitted or shared.
        unsafe { cached.as_ptr().write_bytes(0x7b, cached.len()) };
        for _ in 0..2 {
            let mut output = pool.output(Command::Draw, 64).unwrap();
            let retained = pool.retain(&cached).unwrap();
            pool.attach(&mut output, retained).unwrap();
            // SAFETY: all writes ended; the ring belongs to the active pool.
            let notify = unsafe { pool.submit(output, &mut commands) }.unwrap();
            assert_eq!(notify, command_host.get(4) == 1);
        }
        let first = command_host.command(&pool, 0);
        let second = command_host.command(&pool, 1);
        first.complete(second.id);
        second.complete(0);
        release_host.release(first.id);
        assert_eq!(
            // SAFETY: the simulated host completed both outputs and their resources.
            unsafe { pool.reap(&mut releases, 1) },
            Ok(Reclaimed {
                outputs: 1,
                notify: true
            })
        );
        assert!(!pool.is_empty());
        // SAFETY: the cached token still retains the complete payload.
        assert_eq!(unsafe { cached.as_ptr().add(255).read() }, 0x7b);
        assert_eq!(
            // SAFETY: the pending chain is the remainder of that host completion.
            unsafe { pool.reap(&mut releases, 50) },
            Ok(Reclaimed {
                outputs: 1,
                notify: false
            })
        );
        assert!(!pool.is_empty());
        pool.release(cached).unwrap();
        empty(&mut pool);
    }

    #[test]
    fn publication_restores_release_id_and_encodes_the_command() {
        let mut memory = [0u64; 512];
        let mut pool = pool(&mut memory, None);
        let mut bytes = Bytes([0; 532]);
        let host = HostRing::new(&mut bytes, 32, 16);
        // SAFETY: the mapping is live and has one guest producer.
        let mut ring = unsafe { CommandRing::new(host.base, 532) }.unwrap();
        let mut output = pool.output(Command::Cursor, 24).unwrap();
        pool.payload(&mut output).unwrap().fill(0x5a);
        // SAFETY: this pool and command ring share the live mapping generation.
        unsafe { pool.submit(output, &mut ring) }.unwrap();
        let output = host.command(&pool, 0);
        assert_eq!((output.id, output.command), (1, 3));
        // SAFETY: the host reads the complete published cursor payload.
        assert_eq!(unsafe { output.data.add(23).read() }, 0x5a);
        // SAFETY: the simulated device stops all access before discarding outputs.
        unsafe { pool.discard_pending() };
        empty(&mut pool);
    }

    #[test]
    fn ring_full_and_corrupt_counters_return_the_builder_for_cancellation() {
        for producer in [32, 33] {
            let mut memory = [0u64; 512];
            let mut pool = pool(&mut memory, None);
            let mut bytes = Bytes([0; 532]);
            let host = HostRing::new(&mut bytes, 32, 16);
            host.set(4, producer);
            // SAFETY: the ring mapping is valid; its counters are checked by push.
            let mut ring = unsafe { CommandRing::new(host.base, 532) }.unwrap();
            let mut output = pool.output(Command::Surface, 32).unwrap();
            let resource = pool.resource(Region::Main, 64).unwrap();
            pool.attach(&mut output, resource).unwrap();
            // SAFETY: the ring belongs to the same simulated active mapping.
            let (error, output) = unsafe { pool.submit(output, &mut ring) }.unwrap_err();
            assert_eq!(
                error,
                if producer == 32 {
                    Error::RingFull
                } else {
                    Error::Ring(RingError::CorruptCounters)
                }
            );
            assert!(pool.pending.is_none());
            assert!(!pool.is_empty());
            pool.cancel(output).unwrap();
            empty(&mut pool);
        }
    }

    #[test]
    fn attachment_limit_returns_the_resource_and_cancel_releases_all_references() {
        let mut memory = [0u64; 512];
        let mut pool = pool(&mut memory, None);
        let cached = pool.resource(Region::Main, 64).unwrap();
        let mut output = pool.output(Command::Draw, 64).unwrap();
        for _ in 0..MAX_RESOURCES {
            let resource = pool.retain(&cached).unwrap();
            pool.attach(&mut output, resource).unwrap();
        }
        let resource = pool.retain(&cached).unwrap();
        let (error, resource) = pool.attach(&mut output, resource).unwrap_err();
        assert_eq!(error, Error::TooManyResources);
        pool.release(resource).unwrap();
        pool.cancel(output).unwrap();
        assert!(!pool.is_empty());
        pool.release(cached).unwrap();
        empty(&mut pool);
    }

    #[test]
    fn wrong_pool_preserves_tokens_without_touching_their_nodes() {
        let mut first = [0u64; 512];
        let mut second = [0u64; 512];
        let mut first = pool(&mut first, None);
        let mut second = pool(&mut second, None);
        let resource = first.resource(Region::Main, 8).unwrap();
        assert!(matches!(second.retain(&resource), Err(Error::WrongPool)));
        let (error, resource) = second.release(resource).unwrap_err();
        assert_eq!(error, Error::WrongPool);
        let output = first.output(Command::Draw, 8).unwrap();
        let (error, mut output) = second.cancel(output).unwrap_err();
        assert_eq!(error, Error::WrongPool);
        assert!(matches!(second.payload(&mut output), Err(Error::WrongPool)));
        let (error, resource) = second.attach(&mut output, resource).unwrap_err();
        assert_eq!(error, Error::WrongPool);
        first.attach(&mut output, resource).unwrap();
        first.cancel(output).unwrap();
        empty(&mut first);
        empty(&mut second);
    }

    #[test]
    fn allocation_failure_rolls_back_headers_and_preserves_id_space() {
        let mut memory = [0u64; 512];
        let mut video = [0u64; 8];
        let mut pool = pool(&mut memory, Some(&mut video));
        assert!(matches!(
            pool.resource(Region::Video, 65),
            Err(Error::OutOfMemory)
        ));
        assert!(matches!(
            pool.resource(Region::Main, usize::MAX),
            Err(Error::InvalidSize)
        ));
        assert!(matches!(
            pool.output(Command::Draw, usize::MAX),
            Err(Error::InvalidSize)
        ));
        assert!(matches!(
            pool.output(Command::Draw, 7),
            Err(Error::InvalidSize)
        ));
        assert_eq!(pool.next_id, 1);
        empty(&mut pool);
    }

    #[test]
    fn malformed_host_chains_never_dereference_release_ids_or_double_free() {
        for malformed in [u64::MAX, 1, 2] {
            let mut memory = [0u64; 1024];
            let mut pool = pool(&mut memory, None);
            let mut command_bytes = Bytes([0; 532]);
            let command_host = HostRing::new(&mut command_bytes, 32, 16);
            let mut release_bytes = Bytes([0; 84]);
            let mut release_host = HostRing::new(&mut release_bytes, 8, 8);
            // SAFETY: both ring mappings remain live with one guest endpoint each.
            let (mut commands, mut releases) = unsafe {
                (
                    CommandRing::new(command_host.base, 532).unwrap(),
                    ReleaseRing::new(release_host.base, 84).unwrap(),
                )
            };
            for _ in 0..2 {
                let output = pool.output(Command::Draw, 8).unwrap();
                // SAFETY: both outputs and this ring belong to the same mapping.
                unsafe { pool.submit(output, &mut commands) }.unwrap();
            }
            let first = command_host.command(&pool, 0);
            let second = command_host.command(&pool, 1);
            first.complete(malformed);
            second.complete(first.id);
            release_host.release(if malformed == u64::MAX {
                malformed
            } else {
                first.id
            });
            // SAFETY: the simulated host ends payload access before release.
            let error = unsafe { pool.reap(&mut releases, 50) }.unwrap_err();
            assert_eq!(
                error,
                if malformed == u64::MAX {
                    Error::UnknownRelease
                } else {
                    Error::InvalidChain
                }
            );
            assert!(!pool.is_empty());
            // SAFETY: the malformed simulated device is stopped before cleanup.
            unsafe { pool.discard_pending() };
            empty(&mut pool);
        }
    }
    #[test]
    fn releases_can_remove_any_queue_position_and_old_ids_cannot_release_reused_storage() {
        let mut memory = [0u64; 1024];
        let mut pool = pool(&mut memory, None);
        let mut command_bytes = Bytes([0; 532]);
        let command_host = HostRing::new(&mut command_bytes, 32, 16);
        let mut release_bytes = Bytes([0; 84]);
        let mut release_host = HostRing::new(&mut release_bytes, 8, 8);
        // SAFETY: each live mapping has one guest endpoint.
        let (mut commands, mut releases) = unsafe {
            (
                CommandRing::new(command_host.base, 532).unwrap(),
                ReleaseRing::new(release_host.base, 84).unwrap(),
            )
        };
        for _ in 0..3 {
            let output = pool.output(Command::Draw, 8).unwrap();
            // SAFETY: output and ring belong to the same active mapping.
            unsafe { pool.submit(output, &mut commands) }.unwrap();
        }
        let oldest = command_host.command(&pool, 0);
        let middle = command_host.command(&pool, 1);
        let newest = command_host.command(&pool, 2);
        middle.complete(newest.id);
        newest.complete(oldest.id);
        oldest.complete(0);
        release_host.release(middle.id);
        // SAFETY: the host finished the complete chain. Zero budget must not pop it.
        let result = unsafe { pool.reap(&mut releases, 0) }.unwrap();
        assert_eq!(result.outputs, 0);
        assert_eq!(release_host.get(12), 0);
        // SAFETY: all three outputs were completed by the host above.
        let result = unsafe { pool.reap(&mut releases, 3) }.unwrap();
        assert_eq!(result.outputs, 3);
        empty(&mut pool);
        let output = pool.output(Command::Draw, 8).unwrap();
        // SAFETY: this new command uses the same active pool and ring.
        unsafe { pool.submit(output, &mut commands) }.unwrap();
        let reused = command_host.command(&pool, 3);
        assert_eq!(reused.data, oldest.data);
        assert_ne!(reused.id, oldest.id);
        release_host.release(oldest.id);
        // SAFETY: the simulated host has stopped accessing old completed outputs.
        // An old ID must be rejected before any access to newly reused storage.
        let result = unsafe { pool.reap(&mut releases, 1) };
        assert_eq!(result, Err(Error::UnknownRelease));
        assert!(pool.find(reused.id).is_some());
        // SAFETY: stop the simulated device, including its new pending command.
        unsafe { pool.discard_pending() };
        empty(&mut pool);
    }
}
