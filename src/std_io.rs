use crate::Reader;
use core::cell::RefCell;
use std::fs::File;
use std::io::{Error, Read, Seek, SeekFrom};

/// Adapts a `Read + Seek` source to [`crate::Reader`].
///
/// This wrapper uses interior mutability because ordinary `Read + Seek` APIs
/// require `&mut self` to perform positioned reads.
pub struct ReadSeekReader<T> {
    inner: RefCell<T>,
}

impl<T> ReadSeekReader<T> {
    /// Wraps a `Read + Seek` source.
    #[must_use]
    pub fn new(inner: T) -> Self {
        Self {
            inner: RefCell::new(inner),
        }
    }

    /// Returns the wrapped source.
    #[must_use]
    pub fn into_inner(self) -> T {
        self.inner.into_inner()
    }
}

impl<T> Reader for ReadSeekReader<T>
where
    T: Read + Seek,
{
    type Error = Error;

    fn size(&self) -> Result<u64, Self::Error> {
        let mut inner = self.inner.borrow_mut();
        let current = inner.stream_position()?;
        let end = inner.seek(SeekFrom::End(0))?;
        inner.seek(SeekFrom::Start(current))?;
        Ok(end)
    }

    fn read_exact_at(&self, pos: u64, buf: &mut [u8]) -> Result<(), Self::Error> {
        let mut inner = self.inner.borrow_mut();
        inner.seek(SeekFrom::Start(pos))?;
        inner.read_exact(buf)
    }
}

#[cfg(unix)]
/// Adapts a Unix [`std::fs::File`] using `FileExt::read_exact_at`.
///
/// Unlike [`ReadSeekReader`], this wrapper performs true immutable positioned
/// reads without seeking the file handle.
pub struct UnixFileReader {
    inner: File,
}

#[cfg(unix)]
impl UnixFileReader {
    /// Wraps a file for immutable positioned reads.
    #[must_use]
    pub fn new(inner: File) -> Self {
        Self { inner }
    }

    /// Returns the wrapped file.
    #[must_use]
    pub fn into_inner(self) -> File {
        self.inner
    }
}

#[cfg(unix)]
impl Reader for UnixFileReader {
    type Error = Error;

    fn size(&self) -> Result<u64, Self::Error> {
        Ok(self.inner.metadata()?.len())
    }

    fn read_exact_at(&self, pos: u64, buf: &mut [u8]) -> Result<(), Self::Error> {
        use std::os::unix::fs::FileExt;

        self.inner.read_exact_at(buf, pos)
    }
}
