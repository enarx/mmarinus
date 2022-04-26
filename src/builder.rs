// SPDX-License-Identifier: Apache-2.0

use crate::map::{Kind, Private};

use super::map::Type;
use super::{Error, Map};

use std::io::ErrorKind;
use std::marker::PhantomData;
use std::mem::forget;
use std::os::unix::io::{AsRawFd, RawFd};

pub trait Stage {}

pub enum Address {
    None,
    At(usize),
    Near(usize),
    Onto(usize),
}

pub struct Size<M> {
    pub(crate) prev: M,
    pub(crate) size: usize,
}

pub struct Destination<M> {
    pub(crate) prev: Size<M>,
    pub(crate) addr: Address,
}

pub struct Source<M, K: Kind> {
    prev: Destination<M>,
    fd: RawFd,
    offset: libc::off_t,
    huge: Option<i32>,
    kind: K,
}

impl<M> Stage for Size<M> {}
impl<M> Stage for Destination<M> {}
impl<M, K: Kind> Stage for Source<M, K> {}

/// A builder used to construct a new memory mapping
pub struct Builder<S: Stage>(pub(crate) S);

impl<M> Builder<Size<M>> {
    /// Creates the mapping anywhere in valid memory
    ///
    /// This is equivalent to specifying `NULL` as the address to `mmap()`.
    #[inline]
    pub fn anywhere(self) -> Builder<Destination<M>> {
        Builder(Destination {
            prev: self.0,
            addr: Address::None,
        })
    }

    /// Creates the mapping at the specified address
    ///
    /// This is equivalent to specifying an address with `MAP_FIXED_NOREPLACE` to `mmap()`.
    #[inline]
    pub fn at(self, addr: usize) -> Builder<Destination<M>> {
        Builder(Destination {
            prev: self.0,
            addr: Address::At(addr),
        })
    }

    /// Creates the mapping near the specified address
    ///
    /// This is equivalent to specifying an address with no additional flags to `mmap()`.
    #[inline]
    pub fn near(self, addr: usize) -> Builder<Destination<M>> {
        Builder(Destination {
            prev: self.0,
            addr: Address::Near(addr),
        })
    }

    /// Creates the mapping at the specified address
    ///
    /// This is equivalent to specifying an address with `MAP_FIXED` to `mmap()`.
    ///
    /// # Safety
    ///
    /// This function is unsafe because it can replace existing mappings,
    /// causing memory corruption.
    #[inline]
    pub unsafe fn onto(self, addr: usize) -> Builder<Destination<M>> {
        Builder(Destination {
            prev: self.0,
            addr: Address::Onto(addr),
        })
    }
}

impl<M> Builder<Destination<M>> {
    /// Creates the mapping without any file backing
    ///
    /// This is equivalent to specifying `-1` as the file descriptor, `0` as
    /// the offset and `MAP_ANONYMOUS` in the flags.
    #[inline]
    pub fn anonymously(self) -> Builder<Source<M, Private>> {
        Builder(Source {
            kind: Private,
            prev: self.0,
            huge: None,
            offset: 0,
            fd: -1,
        })
    }

    /// Creates the mapping using the contents of the specified file
    ///
    /// This is equivalent to specifying a valid file descriptor and an offset.
    #[inline]
    pub fn from<U: AsRawFd>(self, file: &mut U, offset: i64) -> Builder<Source<M, Private>> {
        Builder(Source {
            fd: file.as_raw_fd(),
            kind: Private,
            prev: self.0,
            huge: None,
            offset,
        })
    }
}

impl<M, K: Kind> Builder<Source<M, K>> {
    /// Uses huge pages for the mapping
    ///
    /// If `pow = 0`, the kernel will pick the huge page size. Otherwise, if
    /// you wish to specify the huge page size, you should give the power
    /// of two which indicates the page size you want.
    #[inline]
    pub fn with_huge_pages(mut self, pow: u8) -> Self {
        self.0.huge = Some(pow.into());
        self
    }

    /// Uses the specified map kind for map creation
    #[inline]
    pub fn with_kind<X: Kind>(self, kind: X) -> Builder<Source<M, X>> {
        Builder(Source {
            offset: self.0.offset,
            prev: self.0.prev,
            huge: self.0.huge,
            fd: self.0.fd,
            kind,
        })
    }

    /// Creates a mapping with the specified permissions
    ///
    /// The use of `Known` permissions should be preferred to the use of
    /// `Unknown` (i.e. runtime) permissions as this will supply a variety of
    /// useful APIs.
    #[inline]
    pub fn map<T: Type>(self, perms: T) -> Result<Map<T, K>, Error<M>> {
        let einval = ErrorKind::InvalidInput.into();
        let perms = perms.perms();
        let kind = self.0.kind.kind();

        let huge = match self.0.huge {
            Some(x) if x & !libc::MAP_HUGE_MASK != 0 => {
                return Err(Error {
                    map: self.0.prev.prev.prev,
                    err: einval,
                })
            }

            Some(x) => (x << libc::MAP_HUGE_SHIFT) | libc::MAP_HUGETLB,
            None => 0,
        };

        let (addr, fixed) = match self.0.prev.addr {
            Address::None => (0, 0),
            Address::At(a) if a != 0 => (a, libc::MAP_FIXED_NOREPLACE),
            Address::Near(a) if a != 0 => (a, 0),
            Address::Onto(a) if a != 0 => (a, libc::MAP_FIXED),
            _ => {
                return Err(Error {
                    map: self.0.prev.prev.prev,
                    err: einval,
                })
            }
        };

        let anon = match self.0.fd {
            -1 => libc::MAP_ANONYMOUS,
            _ => 0,
        };

        let size = self.0.prev.prev.size;
        let flags = kind | fixed | anon | huge;

        let ret = unsafe { libc::mmap(addr as _, size, perms, flags, self.0.fd, self.0.offset) };
        if ret == libc::MAP_FAILED {
            return Err(Error {
                map: self.0.prev.prev.prev,
                err: std::io::Error::last_os_error(),
            });
        }

        forget(self.0.prev.prev.prev);

        Ok(Map {
            addr: ret as usize,
            size: self.0.prev.prev.size,
            data: PhantomData,
        })
    }
}
