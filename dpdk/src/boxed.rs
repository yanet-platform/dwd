use core::{
    ops::{Deref, DerefMut},
    ptr::NonNull,
};

use std::alloc::Layout;

use crate::ffi;
use crate::{error::Error, SocketId, RTE_CACHE_LINE_SIZE};

#[derive(Debug)]
pub struct RteBox<T> {
    ptr: NonNull<T>,
}

impl<T> RteBox<T> {
    pub fn new(v: T, socket_id: SocketId) -> Result<Self, Error> {
        let ptr = unsafe {
            ffi::rte_malloc_socket(
                core::ptr::null(),
                Layout::new::<T>().size(),
                RTE_CACHE_LINE_SIZE,
                socket_id as i32,
            ) as *mut T
        };
        if ptr.is_null() {
            return Err(Error::new(ffi::ENOMEM as i32));
        }

        unsafe { ptr.write(v) };

        let ptr = NonNull::new(ptr).expect("ptr is not NULL");
        let boxed = Self { ptr };

        Ok(boxed)
    }
}

impl<T> Drop for RteBox<T> {
    fn drop(&mut self) {
        unsafe { ffi::rte_free(self.ptr.as_ptr() as *mut core::ffi::c_void) };
    }
}

impl<T> Deref for RteBox<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { self.ptr.as_ref() }
    }
}

impl<T> DerefMut for RteBox<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.ptr.as_mut() }
    }
}

unsafe impl<T> Send for RteBox<T> {}
