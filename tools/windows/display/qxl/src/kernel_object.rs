// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

//! Stable kernel callback objects with opaque ownership transfer.

use core::ffi::c_void;
use core::mem::{align_of, size_of};
use core::ptr::{NonNull, addr_of, addr_of_mut, drop_in_place};

pub(crate) type Release = unsafe extern "system" fn(*mut c_void);

#[repr(C)]
struct Node<T> {
    allocation: NonNull<u8>,
    release: Release,
    value: T,
}

#[derive(Debug)]
pub(crate) struct Object<T>(NonNull<Node<T>>);

impl<T> Object<T> {
    pub(crate) const fn allocation_bytes() -> Option<usize> {
        size_of::<Node<T>>().checked_add(align_of::<Node<T>>() - 1)
    }

    /// # Safety
    /// Supply a uniquely owned allocation writable for length bytes. Release
    /// must reclaim that original pointer once, after value destruction, in the
    /// same valid kernel context. The allocation must have no other users.
    /// Failure returns value without releasing or changing the allocation.
    pub(crate) unsafe fn new_in(
        allocation: NonNull<u8>,
        length: usize,
        release: Release,
        value: T,
    ) -> Result<Self, T> {
        let Some(address) = allocation
            .as_ptr()
            .addr()
            .checked_add(align_of::<Node<T>>() - 1)
            .map(|address| address & !(align_of::<Node<T>>() - 1))
        else {
            return Err(value);
        };
        let offset = address - allocation.as_ptr().addr();
        if length > isize::MAX as usize
            || offset
                .checked_add(size_of::<Node<T>>())
                .is_none_or(|end| end > length)
            || address.checked_add(size_of::<Node<T>>()).is_none()
        {
            return Err(value);
        }
        let pointer = allocation.as_ptr().with_addr(address).cast::<Node<T>>();
        // SAFETY: alignment and complete extent are checked; the allocation is
        // exclusively owned and no reference or callback can see this node yet.
        unsafe {
            pointer.write(Node {
                allocation,
                release,
                value,
            })
        };
        // SAFETY: address derives from a non-null pointer by checked addition.
        Ok(Self(unsafe { NonNull::new_unchecked(pointer) }))
    }

    pub(crate) fn get(&self) -> &T {
        // SAFETY: this owner retains the initialised, immovable live node.
        unsafe { &*addr_of!((*self.0.as_ptr()).value) }
    }

    pub(crate) fn into_handle(self) -> NonNull<c_void> {
        let handle = self.0.cast();
        core::mem::forget(self);
        handle
    }

    /// # Safety
    /// The handle must come from into_handle for exactly this T and remain
    /// owned by the caller's callback framework for the returned borrow. Never
    /// destroy/recover it while the borrow or any interior user remains live.
    pub(crate) unsafe fn borrow_handle<'a>(handle: NonNull<c_void>) -> &'a T {
        // SAFETY: the framework retains this exact typed live node through 'a.
        unsafe { &*addr_of!((*handle.cast::<Node<T>>().as_ptr()).value) }
    }

    /// # Safety
    /// Recover this exact typed handle once, after all callback/interior users
    /// stop. It must not already have been recovered or destroyed.
    pub(crate) unsafe fn from_handle(handle: NonNull<c_void>) -> Self {
        Self(handle.cast())
    }
}

impl<T> Drop for Object<T> {
    fn drop(&mut self) {
        let node = self.0.as_ptr();
        // SAFETY: this unique owner retains the complete node. Save allocation
        // metadata before dropping T, then reclaim backing storage only after
        // its destructor and all field destructors have finished.
        unsafe {
            let allocation = (*node).allocation;
            let release = (*node).release;
            drop_in_place(addr_of_mut!((*node).value));
            release(allocation.as_ptr().cast());
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use core::mem::{MaybeUninit, offset_of};
    use core::sync::atomic::{AtomicUsize, Ordering};
    use std::{boxed::Box, sync::Arc};

    #[repr(C, align(64))]
    struct Backing {
        order: Arc<AtomicUsize>,
        bytes: [MaybeUninit<u8>; 512],
    }

    #[repr(align(64))]
    #[derive(Debug)]
    struct Value(Arc<AtomicUsize>);

    impl Drop for Value {
        fn drop(&mut self) {
            assert_eq!(self.0.fetch_add(1, Ordering::SeqCst), 0);
        }
    }

    fn allocate(order: &Arc<AtomicUsize>) -> NonNull<u8> {
        let backing = Box::into_raw(Box::new(Backing {
            order: order.clone(),
            bytes: [MaybeUninit::uninit(); 512],
        }));
        // SAFETY: derive from the whole live allocation, with an intentional
        // one-byte displacement. The node must align within the remaining 511.
        unsafe { NonNull::new_unchecked(backing.cast::<u8>().add(offset_of!(Backing, bytes) + 1)) }
    }

    unsafe extern "system" fn release(pointer: *mut c_void) {
        // SAFETY: allocate retained this exact Box and supplies the displaced
        // pointer with its whole-allocation provenance. No value user remains.
        let backing = unsafe {
            Box::from_raw(
                pointer
                    .cast::<u8>()
                    .sub(offset_of!(Backing, bytes) + 1)
                    .cast::<Backing>(),
            )
        };
        assert_eq!(backing.order.fetch_add(1, Ordering::SeqCst), 1);
    }

    #[test]
    fn aligned_value_drops_before_original_allocation_is_reclaimed() {
        let order = Arc::new(AtomicUsize::new(0));
        let memory = allocate(&order);
        // SAFETY: this unique 512-byte storage has 511 bytes after displacement;
        // release reclaims its original backing after the value destructor.
        let object = unsafe { Object::new_in(memory, 511, release, Value(order.clone())) }.unwrap();
        assert_eq!(core::ptr::from_ref(object.get()).addr() % 64, 0);
        assert!(Object::<Value>::allocation_bytes().unwrap() <= 511);
        drop(object);
        assert_eq!(order.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn opaque_handle_transfer_retains_the_same_value_until_recovery() {
        let order = Arc::new(AtomicUsize::new(0));
        let memory = allocate(&order);
        // SAFETY: same unique live allocation and matching destructor contract.
        let object = unsafe { Object::new_in(memory, 511, release, Value(order.clone())) }.unwrap();
        let address = core::ptr::from_ref(object.get());
        let handle = object.into_handle();
        assert_eq!(order.load(Ordering::SeqCst), 0);
        // SAFETY: the framework owns this exact typed handle throughout borrow.
        let borrowed = unsafe { Object::<Value>::borrow_handle(handle) };
        assert_eq!(core::ptr::from_ref(borrowed), address);
        // SAFETY: the borrow ended and no interior user remains; recover once.
        drop(unsafe { Object::<Value>::from_handle(handle) });
        assert_eq!(order.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn short_storage_returns_value_without_taking_allocation_ownership() {
        let order = Arc::new(AtomicUsize::new(0));
        let memory = allocate(&order);
        // SAFETY: the allocation is live and exclusive; its intentionally short
        // supplied extent must fail without releasing/changing backing storage.
        let result = unsafe { Object::new_in(memory, 1, release, Value(order.clone())) };
        assert_eq!(order.load(Ordering::SeqCst), 0);
        let value = result.unwrap_err();
        drop(value);
        assert_eq!(order.load(Ordering::SeqCst), 1);
        // SAFETY: failed construction left this original allocation with us.
        unsafe { release(memory.as_ptr().cast()) };
        assert_eq!(order.load(Ordering::SeqCst), 2);
    }
}
