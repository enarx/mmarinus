// SPDX-License-Identifier: Apache-2.0

//! The `mmarinus` crate wraps the underlying system `mmap()` call in safe semantics.
//!
//! For example:
//!
//! ```rust
//! use mmarinus::{Map, perms};
//! use std::io::Read;
//!
//! let mut hosts = std::fs::File::open("/etc/hosts").unwrap();
//!
//! let mut chunk = [0u8; 32];
//! hosts.read_exact(&mut chunk).unwrap();
//!
//! let map = Map::bytes(32)
//!     .near(128 * 1024 * 1024)
//!     .from(&mut hosts, 0)
//!     .with(perms::Read)
//!     .unwrap();
//!
//! assert_eq!(&*map, &chunk);
//! ```
//!
//! You can also remap an existing mapping:
//!
//! ```rust
//! use mmarinus::{Map, perms};
//! use std::io::Read;
//!
//! let mut hosts = std::fs::File::open("/etc/hosts").unwrap();
//!
//! let mut chunk = [0u8; 32];
//! hosts.read_exact(&mut chunk).unwrap();
//!
//! let mut map = Map::bytes(32)
//!     .anywhere()
//!     .from(&mut hosts, 0)
//!     .with(perms::Read)
//!     .unwrap();
//!
//! assert_eq!(&*map, &chunk);
//!
//! let mut map = map.remap()
//!     .from(&mut hosts, 0)
//!     .with(perms::ReadWrite)
//!     .unwrap();
//!
//! assert_eq!(&*map, &chunk);
//!
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
//! use std::io::Read;
//!
//! let mut hosts = std::fs::File::open("/etc/hosts").unwrap();
//!
//! let mut chunk = [0u8; 32];
//! hosts.read_exact(&mut chunk).unwrap();
//!
//! let map = Map::bytes(32)
//!     .near(128 * 1024 * 1024)
//!     .from(&mut hosts, 0)
//!     .with(perms::Read)
//!     .unwrap();
//!
//! assert_eq!(&*map, &chunk);
//!
//! let mut map = map.reprotect(perms::ReadWrite).unwrap();
//! assert_eq!(&*map, &chunk);
//!
//! for i in map.iter_mut() {
//!     *i = 255;
//! }
//! assert_eq!(&*map, &[255; 32]);
//! ```
//!
//! Mapping a whole file into memory is easy:
//!
//! ```rust
//! use mmarinus::{Map, Private, perms};
//!
//! let map = Map::load("/etc/hosts", Private, perms::Read).unwrap();
//! ```

#![forbid(clippy::expect_used, clippy::panic)]
#![warn(
    rust_2018_idioms,
    unused_lifetimes,
    unused_qualifications,
    clippy::all,
    missing_docs
)]

mod builder;
mod error;
mod map;

pub use error::Error;
pub use map::{Map, Private, Shared};
pub mod perms;
