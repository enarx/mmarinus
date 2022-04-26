use super::builder::{Address, Builder, Destination, Size};
use super::{perms, Error};

use std::convert::{TryFrom, TryInto};
use std::io::ErrorKind;
use std::marker::PhantomData;
use std::mem::forget;
use std::path::Path;
use std::slice::{from_raw_parts, from_raw_parts_mut};

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

/// A smart pointer to a mapped region of memory
///
/// When this reference is destroyed, `munmap()` will be called on the region.
#[derive(Debug)]
pub struct Map<T: Type> {
    pub(crate) addr: usize,
    pub(crate) size: usize,
    pub(crate) data: PhantomData<T>,
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
