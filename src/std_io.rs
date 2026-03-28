use crate::{Archive, Entry, Reader};
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

impl<T> From<T> for ReadSeekReader<T> {
    fn from(inner: T) -> Self {
        Self::new(inner)
    }
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
impl From<File> for UnixFileReader {
    fn from(inner: File) -> Self {
        Self::new(inner)
    }
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

#[cfg(unix)]
/// Platform-optimized reader for [`std::fs::File`].
pub type FileReader = UnixFileReader;

#[cfg(not(unix))]
/// Platform-optimized reader for [`std::fs::File`].
pub type FileReader = ReadSeekReader<File>;

/// A [`Read`] adapter over an entry's payload bytes.
///
/// Created by [`Entry::reader`]. Each [`read`](Read::read) call performs a
/// positioned read on the underlying archive, so the reader is not buffered.
pub struct EntryReader<'a, R> {
    archive: &'a Archive<R>,
    pos: u64,
    end: u64,
}

impl<'a, R: Reader> Read for EntryReader<'a, R>
where
    R::Error: Into<Error>,
{
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let remaining = (self.end - self.pos) as usize;
        let to_read = buf.len().min(remaining);
        if to_read == 0 {
            return Ok(0);
        }
        self.archive
            .reader
            .read_exact_at(self.pos, &mut buf[..to_read])
            .map_err(Into::into)?;
        self.pos += to_read as u64;
        Ok(to_read)
    }
}

impl<'a, R: Reader> Entry<'a, R> {
    /// Returns a [`Read`] adapter over this entry's payload bytes.
    ///
    /// The adapter reads raw stored or compressed bytes from the archive;
    /// decompression is the caller's responsibility.
    ///
    /// # Errors
    ///
    /// Returns a structural [`crate::Error`] if the local header is malformed
    /// or the data range extends past the archive.
    pub fn reader(&self) -> Result<EntryReader<'a, R>, crate::Error<R::Error>> {
        let range = self.data_range()?;
        Ok(EntryReader {
            archive: self.archive,
            pos: range.data_range.start,
            end: range.data_range.end,
        })
    }
}
