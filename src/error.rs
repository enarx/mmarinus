// SPDX-License-Identifier: Apache-2.0

/// The error condition
///
/// This type is mostly a wrapper for `std::io::Error` with one additional
/// feature: it conveys ownership to a mapping. This enables the pattern
/// where an old mapping is valid until the conversion operation is successful.
/// If the operation is unsuccessful, the old mapping is returned along with
/// the error condition.
#[derive(Debug)]
pub struct Error<M> {
    /// The previous mapping that could not be modified
    pub map: M,

    /// The underlying error
    pub err: std::io::Error,
}

impl<M> std::fmt::Display for Error<M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        self.err.fmt(f)
    }
}

impl<M: std::fmt::Debug> std::error::Error for Error<M> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.err)
    }
}

impl<M> From<Error<M>> for std::io::Error {
    fn from(value: Error<M>) -> Self {
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

impl From<std::io::ErrorKind> for Error<()> {
    fn from(value: std::io::ErrorKind) -> Self {
        Self {
            map: (),
            err: value.into(),
        }
    }
}
