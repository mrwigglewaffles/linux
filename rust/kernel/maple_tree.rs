// SPDX-License-Identifier: GPL-2.0

//! Maple trees.
//!
//! C header: [`include/linux/maple_tree.h`](srctree/include/linux/maple_tree.h)
//!
//! Reference: <https://docs.kernel.org/core-api/maple_tree.html>
//!
//! # Example
//! ```
//! # use kernel::maple_tree::*;
//! # use kernel::alloc::{KBox, flags::GFP_KERNEL};
//! let mtree = MapleTree::new(flags::DEFAULT_TREE);
//! let mut guard = mtree.lock();
//! let entry = KBox::new(5, GFP_KERNEL)?;
//! guard.insert_range(0, 10, entry, GFP_KERNEL);
//!
//! for i in 0..10{
//!    assert_eq!(guard.get(i), Some(&5));
//! }
//!
//! guard.remove(2);
//!
//! for i in 0..10{
//!    assert_eq!(guard.get(i), None);
//! }
//!
//! # Ok::<(), Error>(())
//! ```

use core::{ffi::c_void, marker::PhantomData, pin::Pin, ptr::NonNull};

use macros::pin_data;
use macros::pinned_drop;

use crate::prelude::PinInit;

use crate::{
    alloc, bindings,
    error::{self, Error},
    pin_init,
    types::{ForeignOwnable, NotThreadSafe, Opaque},
};

/// A `MapleTree` is a tree like data structure that is optimized for storing
/// non-overlaping ranges and mapping them to pointers.
///
/// # Invariants
///
/// self.root is always a valid and initialized `bindings::maple_tree`
/// all values inserted into the tree come from `T::into_foreign`
#[pin_data(PinnedDrop)]
pub struct MapleTree<T: ForeignOwnable> {
    #[pin]
    root: Opaque<bindings::maple_tree>,
    _p: PhantomData<T>,
}

impl<T: ForeignOwnable> MapleTree<T> {
    /// creates a new `MapleTree` with the specified `flags`
    ///
    /// see [`flags`] for the list of flags and their usage
    pub fn new(flags: Flags) -> impl PinInit<Self> {
        pin_init!(
            Self{
                // SAFETY:
                // - mt is valid because of ffi_init
                // - maple_tree contains a lock which should be pinned
                root <- Opaque::ffi_init(|mt| unsafe {
                    bindings::mt_init_flags(mt, flags.as_raw())
                }),
                _p: PhantomData
            }

        )
    }

    fn iter(&self) -> Iter<'_, T> {
        // SAFETY:
        // self.root.get() will allways point to a valid maple_tree
        // by the invariants of MapleTree
        let ma_state = unsafe { Opaque::new(bindings::MA_STATE(self.root.get(), 0, usize::MAX)) };
        Iter {
            ma_state,
            _p: PhantomData,
        }
    }

    /// locks the `Mapletree`'s internal mutex and returns a [`Guard`].
    /// When the `Guard` is dropped, the internal mutex is unlocked
    pub fn lock(&self) -> Guard<'_, T> {
        // SAFETY:
        // self.root.get() will allways point to a valid maple_tree
        // by the invariants of MapleTree
        unsafe { bindings::mtree_lock(self.root.get()) };
        Guard {
            tree: self,
            _not_send: NotThreadSafe,
        }
    }
}

#[pinned_drop]
impl<T: ForeignOwnable> PinnedDrop for MapleTree<T> {
    fn drop(self: Pin<&mut Self>) {
        for i in self.iter() {
            //SAFETY:
            // - we can call from_foreign because all values inserted into a MapleTree
            //   come from T::into_foreign
            // - i.as_ptr is guaranteed to not be null because of the invariant of NonNull
            // - we have exclusive access to self because we should have
            //   exclussive access whenever drop is called
            let original = unsafe { T::from_foreign(i.as_ptr()) };
            drop(original);
        }
        // SAFETY:
        // - self.root.get() will allways point to a valid maple_tree
        //   by the invariants of MapleTree
        // - we can call this without taking the mutex because we should have
        //   exclusive access whenever drop is called
        unsafe {
            bindings::__mt_destroy(self.root.get());
        }
    }
}

/// an iterator over all of the values in a [`MapleTree`].
/// all returned pointers are guaranteed to have been inserted by the user
/// but the pointers are not guaranteedto be still be valid
/// another thread may have already removed and dropped the pointers
/// so to safely deref the returned pointers the user must
/// have exclusive write access to the `MapleTree`
struct Iter<'a, T: ForeignOwnable> {
    ma_state: Opaque<bindings::ma_state>,
    _p: PhantomData<&'a MapleTree<T>>,
}

impl<'a, T: ForeignOwnable> Iterator for Iter<'a, T> {
    type Item = NonNull<c_void>;
    fn next(&mut self) -> Option<Self::Item> {
        // SAFETY:
        // self.ma_state.get() will allways point to a valid ma_state by the invariants of Iter
        let ptr: *mut c_void = unsafe { bindings::mas_find(self.ma_state.get(), usize::MAX) };
        NonNull::new(ptr)
    }
}

/// A lock guard for a [`MapleTree`]
///
/// The lock is unlocked when the guard goes out of scope
///
/// # Invariants
///
/// `tree` is always a valid refrence to a locked `MapleTree`
/// `tree` is unlocked when the guard is dropped
pub struct Guard<'a, T: ForeignOwnable> {
    tree: &'a MapleTree<T>,
    _not_send: NotThreadSafe,
}

impl<'a, T: ForeignOwnable> Guard<'a, T> {
    /// Removes a value at the specified index.
    /// if there is no value at the index returns `None`.
    /// if there is a value at the index returns it and unmaps the entire range
    pub fn remove(&mut self, index: usize) -> Option<T> {
        // SAFETY:
        // - we can safely call mas_erase because
        // - we can call try_from_foreign because all values inserted into a MapleTree
        //   come from T::into_foreign
        unsafe {
            let removed = self.map_at_index(index, |ptr| bindings::mas_erase(ptr));
            T::try_from_foreign(removed)
        }
    }

    /// Returns a refrence to the `T` at `index` if it exists,
    /// otherwise returns `None`
    pub fn get(&self, index: usize) -> Option<T::Borrowed<'_>> {
        // SAFETY:
        // self.tree.root.get() will always be valid because of the invariants of MapleTree
        let ptr = unsafe { bindings::mtree_load(self.tree.root.get(), index) };
        if ptr.is_null() {
            return None;
        }
        // SAFETY:
        // - we can safely call borrow because all values inserted into a MapleTree
        //   come from T::into_foreign
        // - ptr is not null by the check above
        Some(unsafe { T::borrow(ptr) })
    }

    /// Returns a mut refrence to the `T` at `index` if it exists,
    /// otherwise returns `None`
    pub fn get_mut(&mut self, index: usize) -> Option<T::BorrowedMut<'_>> {
        // SAFETY:
        // self.tree.root.get() will always be valid because of the invariants of MapleTree
        let ptr = unsafe { bindings::mtree_load(self.tree.root.get(), index) };
        if ptr.is_null() {
            return None;
        }
        // SAFETY:
        // - we can safely call borrowmut because all values inserted into a MapleTree
        //   come from T::into_foreign
        // - ptr is not null by the check above
        // - we have exclusive ownership becauce this function takes `&mut self`
        Some(unsafe { T::borrow_mut(ptr) })
    }

    /// a convenience alias for [`Self::insert_range`] where `start == end`
    pub fn insert(&mut self, index: usize, value: T, gfp: alloc::Flags) -> Result<(), (T, Error)> {
        self.insert_range(index, index, value, gfp)
    }

    /// Maps the range `[start, end]` to `value` in the MapleTree.
    /// if `[start, end]` overlaps with any range already inserted, then `value` will
    /// not be inserted.
    pub fn insert_range(
        &mut self,
        start: usize,
        end: usize,
        value: T,
        gfp: alloc::Flags,
    ) -> Result<(), (T, Error)> {
        let ptr = value.into_foreign();

        // SAFETY:
        // - we can call __mtree_insert_range because we hold the lock because of the
        //   invariants of self
        // - self.tree.root.get() will always be valid because of the invariants of MapleTree
        let errno = unsafe {
            bindings::__mtree_insert_range(self.tree.root.get(), start, end, ptr, gfp.as_raw())
        };

        let err = error::to_result(errno);
        // SAFETY:
        // - we can call from_foreign because all values inserted into a MapleTree
        //   come from T::into_foreign
        // - we have exclusive ownership of ptr because if err is an error then, ptr was
        //   not inserted into the MapleTree
        //
        err.map_err(|e| unsafe { (T::from_foreign(ptr), e) })
    }

    /// helper function for internal use when using the advanced maple_tree api
    /// calls `f` on a ma_state that contains the range `[index, index]`
    fn map_at_index<U>(&self, index: usize, f: impl FnOnce(*mut bindings::ma_state) -> U) -> U {
        // SAFETY:
        // start <= end will always be true because start == end
        unsafe { self.map_at_range(index, index, f) }
    }

    /// helper function for internal use when using the advanced maple_tree api
    /// calls `f` on a ma_state that contains the range `[start, end]`
    /// # Safety
    /// it is up to the callers to ensure that start <= end
    unsafe fn map_at_range<U>(
        &self,
        start: usize,
        end: usize,
        f: impl FnOnce(*mut bindings::ma_state) -> U,
    ) -> U {
        // SAFETY:
        // - self.tree.root.get() will always be valid by the invariants of self
        // - it is the callers responsibility to unsure that start <= end
        let mas: MapleState<'_, T> = unsafe { MapleState::new(self.tree.root.get(), start, end) };
        f(mas.get())
    }
}

impl<T: ForeignOwnable> Drop for Guard<'_, T> {
    fn drop(&mut self) {
        // SAFETY:
        // - unlock the MapleTree because the guard is being dropped
        // - self.tree.root.get() will always be valid because of the invariants of MapleTree
        unsafe { bindings::mtree_unlock(self.tree.root.get()) };
    }
}

struct MapleState<'a, T: ForeignOwnable> {
    inner: Opaque<bindings::ma_state>,
    _p: PhantomData<&'a MapleTree<T>>,
}

impl<'a, T: ForeignOwnable> MapleState<'a, T> {
    /// creates a new MapleState
    /// # Safety
    /// it is up to the callers to guarantee that
    /// tree is valid and
    /// start <= end
    unsafe fn new(tree: *mut bindings::maple_tree, start: usize, end: usize) -> Self {
        // Safety: guaranteed by caller
        let mas = Opaque::new(unsafe { bindings::MA_STATE(tree, start, end) });
        Self {
            inner: mas,
            _p: PhantomData,
        }
    }

    fn get(&self) -> *mut bindings::ma_state {
        self.inner.get()
    }
}

#[derive(Clone, Copy, PartialEq)]
/// flags to be used with [`MapleTree::new`].
///
/// values can used from the [`flags`] module.
pub struct Flags(u32);

impl Flags {
    pub(crate) fn as_raw(self) -> u32 {
        self.0
    }
}

/// flags to be used with [`MapleTree::new`]
pub mod flags {
    use super::Flags;

    /// Creates a default MapleTree
    pub const DEFAULT_TREE: Flags = Flags(0);

    /// if a `MapleTree` is created with ALLOC_TREE the `MapleTree` will be a alloc tree.
    /// alloc trees have a lower branching factor but allows the user to search
    /// for a gap of a given size.
    pub const ALLOC_TREE: Flags = Flags(bindings::MT_FLAGS_ALLOC_RANGE);
}
