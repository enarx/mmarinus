// SPDX-License-Identifier: Apache-2.0

//! The `mmarinus` crate wraps the underlying system `mmap()` call in safe semantics.
//!
//! For example:
//!
//! ```rust
//! use mmarinus::{Map, perms};
//!
//! let mut zero = std::fs::File::open("/dev/zero").unwrap();
//!
//! let map = Map::map(32)
//!     .near(128 * 1024 * 1024)
//!     .from(&mut zero, 0)
//!     .map(perms::Read)
//!     .unwrap();
//!
//! assert_eq!(&*map, &[0; 32]);
//! ```
//!
//! You can also remap an existing mapping:
//!
//! ```rust
//! use mmarinus::{Map, perms};
//!
//! let mut zero = std::fs::File::open("/dev/zero").unwrap();
//!
//! let mut map = Map::map(32)
//!     .anywhere()
//!     .from(&mut zero, 0)
//!     .map(perms::Read)
//!     .unwrap();
//!
//! assert_eq!(&*map, &[0; 32]);
//!
//! let mut map = map.remap()
//!     .from(&mut zero, 0)
//!     .map(perms::ReadWrite)
//!     .unwrap();
//!
//! assert_eq!(&*map, &[0; 32]);
//! for i in map.iter_mut() {
//!     *i = 255;
//! }
//! assert_eq!(&*map, &[255; 32]);
//! ```
//!
//! Alternatively, you can just change the permissions:
//!
//! ```rust
//! use mmarinus::{Map, perms};
//!
//! let mut zero = std::fs::File::open("/dev/zero").unwrap();
//!
//! let mut map = Map::map(32)
//!     .at(128 * 1024 * 1024)
//!     .from(&mut zero, 0)
//!     .map(perms::Read)
//!     .unwrap();
//!
//! assert_eq!(&*map, &[0; 32]);
//!
//! let mut map = map.reprotect(perms::ReadWrite).unwrap();
//!
//! assert_eq!(&*map, &[0; 32]);
//! for i in map.iter_mut() {
//!     *i = 255;
//! }
//! assert_eq!(&*map, &[255; 32]);
//! ```
//!
//! Mapping a whole file into memory is easy:
//!
//! ```rust
//! use mmarinus::{Map, perms};
//!
//! let map = Map::load("/etc/os-release", perms::Read).unwrap();
//! ```

#![deny(clippy::all)]
#![deny(missing_docs)]

use std::convert::{TryFrom, TryInto};
use std::io::ErrorKind;
use std::marker::PhantomData;
use std::mem::forget;
use std::os::unix::io::{AsRawFd, RawFd};
use std::path::Path;
use std::slice::{from_raw_parts, from_raw_parts_mut};

mod sealed {
    pub trait Stage {}

    pub trait Type {
        fn perms(self) -> libc::c_int;
    }

    pub trait Known: Type {
        const VALUE: libc::c_int;
    }

    impl<T: Known> Type for T {
        fn perms(self) -> libc::c_int {
            Self::VALUE
        }
    }

    pub trait Readable: Known {}

    pub trait Writeable: Known {}

    pub trait Executable: Known {}
}

use sealed::*;

/// Permissions for a mapping
pub mod perms {
    #![allow(missing_docs)]

    macro_rules! perm {
        ($($name:ident[$($trait:ident),* $(,)?] => $value:expr),+ $(,)?) => {
            $(
                #[derive(Debug)]
                pub struct $name;

                impl super::Known for $name {
                    const VALUE: libc::c_int = $value;
                }

                $(
                    impl super::$trait for $name {}
                )*
            )+
        };
    }

    perm! {
        None[] => libc::PROT_NONE,
        Read[Readable] => libc::PROT_READ,
        Write[Writeable] => libc::PROT_WRITE,
        Execute[Executable] => libc::PROT_EXEC,
        ReadWrite[Readable, Writeable] => libc::PROT_READ | libc::PROT_WRITE,
        ReadExecute[Readable, Executable] => libc::PROT_READ | libc::PROT_EXEC,
        WriteExecute[Writeable, Executable] => libc::PROT_WRITE | libc::PROT_EXEC,
        ReadWriteExecute[Readable, Writeable, Executable] => libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC,
    }

    pub struct Unknown(pub libc::c_int);

    impl super::Type for Unknown {
        #[inline]
        fn perms(self) -> libc::c_int {
            self.0
        }
    }
}

enum Address {
    None,
    At(usize),
    Near(usize),
    Onto(usize),
}

/// The error condition
///
/// This type is mostly a wrapper for `std::io::Error` with one additional
/// feature: it conveys ownership to a mapping. This enables the pattern
/// where an old mapping is valid until the conversion operation is successful.
/// If the operation is unsuccessful, the old mapping is returned along with
/// the error condition.
#[derive(Debug)]
pub struct Error<T> {
    /// The previous mapping that could not be modified
    pub map: T,

    /// The underlying error
    pub err: std::io::Error,
}

impl<T> std::fmt::Display for Error<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> Result<(), std::fmt::Error> {
        self.err.fmt(f)
    }
}

impl<T: std::fmt::Debug> std::error::Error for Error<T> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.err)
    }
}

impl<T> From<Error<T>> for std::io::Error {
    fn from(value: Error<T>) -> Self {
        value.err
    }
}

impl From<std::io::Error> for Error<()> {
    fn from(value: std::io::Error) -> Self {
        Self {
            map: (),
            err: value,
        }
    }
}

impl From<ErrorKind> for Error<()> {
    fn from(value: ErrorKind) -> Self {
        Self {
            map: (),
            err: value.into(),
        }
    }
}

#[doc(hidden)]
pub struct Size<T> {
    prev: T,
    size: usize,
}

#[doc(hidden)]
pub struct Destination<T> {
    prev: Size<T>,
    addr: Address,
}

#[doc(hidden)]
pub struct Source<'a, T> {
    prev: Destination<T>,
    fd: RawFd,
    offset: libc::off_t,
    huge: Option<i32>,
    data: PhantomData<&'a ()>,
}

impl<T> Stage for Size<T> {}
impl<T> Stage for Destination<T> {}
impl<'a, T> Stage for Source<'a, T> {}

/// A builder used to construct a new memory mapping
pub struct Builder<T: Stage>(T);

impl<T> Builder<Size<T>> {
    /// Creates the mapping anywhere in valid memory
    ///
    /// This is equivalent to specifying `NULL` as the address to `mmap()`.
    #[inline]
    pub fn anywhere(self) -> Builder<Destination<T>> {
        Builder(Destination {
            prev: self.0,
            addr: Address::None,
        })
    }

    /// Creates the mapping at the specified address
    ///
    /// This is equivalent to specifying an address with `MAP_FIXED_NOREPLACE` to `mmap()`.
    #[inline]
    pub fn at(self, addr: usize) -> Builder<Destination<T>> {
        Builder(Destination {
            prev: self.0,
            addr: Address::At(addr),
        })
    }

    /// Creates the mapping near the specified address
    ///
    /// This is equivalent to specifying an address with no additional flags to `mmap()`.
    #[inline]
    pub fn near(self, addr: usize) -> Builder<Destination<T>> {
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
    pub unsafe fn onto(self, addr: usize) -> Builder<Destination<T>> {
        Builder(Destination {
            prev: self.0,
            addr: Address::Onto(addr),
        })
    }
}

impl<T> Builder<Destination<T>> {
    /// Creates the mapping without any file backing
    ///
    /// This is equivalent to specifying `-1` as the file descriptor, `0` as
    /// the offset and `MAP_ANONYMOUS` in the flags.
    #[inline]
    pub fn anonymously<'a>(self) -> Builder<Source<'a, T>> {
        Builder(Source {
            data: PhantomData,
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
    pub fn from<U: AsRawFd>(self, file: &mut U, offset: i64) -> Builder<Source<T>> {
        Builder(Source {
            fd: file.as_raw_fd(),
            data: PhantomData,
            prev: self.0,
            huge: None,
            offset,
        })
    }
}

impl<'a, T> Builder<Source<'a, T>> {
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

    /// Creates a mapping with the specified permissions
    ///
    /// The use of `Known` permissions should be preferred to the use of
    /// `Unknown` (i.e. runtime) permissions as this will supply a variety of
    /// useful APIs.
    #[inline]
    pub fn map<U: Type>(self, perms: U) -> Result<Map<U>, Error<T>> {
        let einval = ErrorKind::InvalidInput.into();
        let perms = perms.perms();

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
        let flags = libc::MAP_PRIVATE | fixed | anon | huge;

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

/// A smart pointer to a mapped region of memory
///
/// When this reference is destroyed, `munmap()` will be called on the region.
#[derive(Debug)]
pub struct Map<T: Type> {
    addr: usize,
    size: usize,
    data: PhantomData<T>,
}

impl<T: Type> Drop for Map<T> {
    fn drop(&mut self) {
        if self.size > 0 {
            unsafe {
                libc::munmap(self.addr as *mut _, self.size);
            }
        }
    }
}

impl<T: Readable> std::ops::Deref for Map<T> {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &[u8] {
        unsafe { from_raw_parts(self.addr as *const u8, self.size) }
    }
}

impl<T: Readable + Writeable> std::ops::DerefMut for Map<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut [u8] {
        unsafe { from_raw_parts_mut(self.addr as *mut u8, self.size) }
    }
}

impl<T: Readable> AsRef<[u8]> for Map<T> {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        &*self
    }
}

impl<T: Readable + Writeable> AsMut<[u8]> for Map<T> {
    #[inline]
    fn as_mut(&mut self) -> &mut [u8] {
        &mut *self
    }
}

impl<T: Known> From<Map<T>> for Map<perms::Unknown> {
    #[inline]
    fn from(value: Map<T>) -> Map<perms::Unknown> {
        let map = Map {
            addr: value.addr,
            size: value.size,
            data: PhantomData,
        };
        forget(value);
        map
    }
}

impl<T: Type> Map<T> {
    /// Maps a whole file into memory
    ///
    /// This is simply a convenience function.
    #[inline]
    pub fn load<U: AsRef<Path>>(path: U, perms: T) -> Result<Map<T>, Error<()>> {
        let err = Err(ErrorKind::InvalidData);
        let mut file = std::fs::File::open(path)?;
        let size = file.metadata()?.len().try_into().or(err)?;
        Map::map(size).anywhere().from(&mut file, 0).map(perms)
    }

    /// Gets the address of the mapping
    #[inline]
    pub fn addr(&self) -> usize {
        self.addr
    }

    /// Gets the size of the mapping
    #[inline]
    pub fn size(&self) -> usize {
        self.size
    }

    /// Changes the settings of an existing mapping
    ///
    /// Upon success, the new mapping "steals" the mapping from the old `Map`
    /// instance. Using the old instance is a logic error, but is safe.
    #[inline]
    pub fn remap(self) -> Builder<Destination<Map<T>>> {
        Builder(Destination {
            addr: Address::Onto(self.addr),
            prev: Size {
                size: self.size,
                prev: self,
            },
        })
    }

    /// Changes the permissions of an existing mapping
    ///
    /// Upon success, the new mapping "steals" the mapping from the old `Map`
    /// instance. Using the old instance is a logic error, but is safe.
    #[inline]
    pub fn reprotect<U: Type>(self, perms: U) -> Result<Map<U>, Error<Self>> {
        if unsafe { libc::mprotect(self.addr as _, self.size, perms.perms()) } != 0 {
            return Err(Error {
                map: self,
                err: std::io::Error::last_os_error(),
            });
        }

        let map = Map {
            addr: self.addr,
            size: self.size,
            data: PhantomData,
        };

        forget(self);
        Ok(map)
    }

    /// Split a mapping at the specified offset.
    ///
    /// The split address MUST be page-aligned or this call will fail.
    ///
    /// # Example
    /// ```
    /// use mmarinus::{Map, perms};
    ///
    /// const SIZE: usize = 4 * 1024 * 1024;
    ///
    /// let map = Map::map(SIZE * 2)
    ///     .anywhere()
    ///     .anonymously()
    ///     .map(perms::Read)
    ///     .unwrap();
    ///
    /// let (l, r) = map.split(SIZE).unwrap();
    /// assert_eq!(l.size(), SIZE);
    /// assert_eq!(r.size(), SIZE);
    /// ```
    pub fn split(self, offset: usize) -> Result<(Self, Self), Error<Self>> {
        if let Ok(psize) = usize::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }) {
            let addr = self.addr + offset;
            if offset <= self.size && addr % psize == 0 {
                let l = Self {
                    addr: self.addr,
                    size: offset,
                    data: PhantomData,
                };

                let r = Self {
                    addr,
                    size: self.size - offset,
                    data: PhantomData,
                };

                forget(self);
                return Ok((l, r));
            }
        }

        Err(Error {
            map: self,
            err: std::io::Error::from_raw_os_error(libc::EINVAL),
        })
    }

    /// Split a mapping at the specified address.
    ///
    /// The address (`at`) MUST be page-aligned or this call will fail.
    ///
    /// # Example
    /// ```
    /// use mmarinus::{Map, perms};
    ///
    /// const SIZE: usize = 4 * 1024 * 1024;
    ///
    /// let map = Map::map(SIZE * 2)
    ///     .anywhere()
    ///     .anonymously()
    ///     .map(perms::Read)
    ///     .unwrap();
    ///
    /// let at = map.addr() + SIZE;
    /// let (l, r) = map.split_at(at).unwrap();
    /// assert_eq!(l.size(), SIZE);
    /// assert_eq!(r.size(), SIZE);
    /// ```
    #[inline]
    pub fn split_at(self, addr: usize) -> Result<(Self, Self), Error<Self>> {
        let offset = match addr >= self.addr {
            false => self.size,
            true => addr - self.addr,
        };

        self.split(offset)
    }
}

impl Map<perms::Unknown> {
    /// Begin creating a mapping of the specified size
    #[inline]
    pub fn map(size: usize) -> Builder<Size<()>> {
        Builder(Size { prev: (), size })
    }
}

#[cfg(test)]
mod tests {
    use crate::{perms, Map};

    #[test]
    fn zero_split() {
        const SIZE: usize = 4 * 1024 * 1024;

        let map = Map::map(SIZE)
            .anywhere()
            .anonymously()
            .map(perms::Read)
            .unwrap();

        let at = map.addr();
        let (l, r) = map.split_at(at).unwrap();
        assert_eq!(l.size(), 0);
        assert_eq!(r.size(), SIZE);
    }

    #[test]
    fn full_size_split() {
        const SIZE: usize = 4 * 1024 * 1024;

        let map = Map::map(SIZE)
            .anywhere()
            .anonymously()
            .map(perms::Read)
            .unwrap();

        let at = map.addr() + SIZE;
        let (l, r) = map.split_at(at).unwrap();
        assert_eq!(l.size(), SIZE);
        assert_eq!(r.size(), 0);
    }
}
