#![no_std]
#![deny(missing_docs)]
//! Low-level `no_std`, `no_alloc` ZIP navigation.
//!
//! The crate does not decompress data. It locates the central directory,
//! iterates entries, and reports raw archive byte ranges for names, metadata,
//! local headers, and payload bytes.
#![cfg_attr(test, allow(clippy::uninlined_format_args))]

#[cfg(feature = "std")]
extern crate std;

use core::convert::TryFrom;
use core::fmt;
use core::marker::PhantomData;
use core::ops::Range;

const EOCD_SIGNATURE: u32 = 0x0605_4B50;
const ZIP64_EOCD_SIGNATURE: u32 = 0x0606_4B50;
const ZIP64_LOCATOR_SIGNATURE: u32 = 0x0706_4B50;
const CENTRAL_HEADER_SIGNATURE: u32 = 0x0201_4B50;
const LOCAL_HEADER_SIGNATURE: u32 = 0x0403_4B50;

const EOCD_LEN: usize = 22;
const ZIP64_LOCATOR_LEN: usize = 20;
const CENTRAL_HEADER_LEN: usize = 46;
const LOCAL_HEADER_LEN: usize = 30;
const MAX_EOCD_SCAN: usize = EOCD_LEN + u16::MAX as usize;
const PATH_SCAN_CHUNK_LEN: usize = 64;

/// Random-access byte source used by [`Archive`].
///
/// Implementations must be able to report a stable total size and fill the
/// requested buffer from an absolute byte position.
pub trait Reader {
    /// Backend-specific I/O error type.
    type Error;

    /// Returns the total archive size in bytes.
    ///
    /// The value must remain valid for subsequent reads.
    ///
    /// # Errors
    ///
    /// Returns any backend error produced while querying the data source size.
    fn size(&self) -> Result<u64, Self::Error>;
    /// Fills `buf` from the absolute byte position `pos`.
    ///
    /// Implementations should return an error rather than short-read. `pos` is
    /// relative to the start of the whole archive, not to any entry.
    ///
    /// # Errors
    ///
    /// Returns any backend error produced while reading at `pos`.
    fn read_exact_at(&self, pos: u64, buf: &mut [u8]) -> Result<(), Self::Error>;
}

/// Error returned by the built-in [`Reader`] implementation for byte slices.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SliceReaderError {
    /// The requested position or byte range is outside the slice.
    OutOfBounds,
}

impl fmt::Display for SliceReaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfBounds => f.write_str("byte range out of bounds"),
        }
    }
}

impl Reader for &[u8] {
    type Error = SliceReaderError;

    fn size(&self) -> Result<u64, Self::Error> {
        Ok(self.len() as u64)
    }

    fn read_exact_at(&self, pos: u64, buf: &mut [u8]) -> Result<(), Self::Error> {
        let pos = usize::try_from(pos).map_err(|_| SliceReaderError::OutOfBounds)?;
        let end = pos
            .checked_add(buf.len())
            .ok_or(SliceReaderError::OutOfBounds)?;
        let src = self.get(pos..end).ok_or(SliceReaderError::OutOfBounds)?;
        buf.copy_from_slice(src);
        Ok(())
    }
}

#[cfg(feature = "std")]
/// `std` adapters for [`Reader`].
pub mod std_io;

/// ZIP compression methods exposed by the central directory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Compression {
    /// Method 0: the data bytes are stored verbatim.
    Stored,
    /// Method 8: the data bytes are deflate-compressed.
    Deflated,
}

impl Compression {
    /// Converts a raw ZIP method id into the low-level enum used by the crate.
    #[must_use]
    pub fn from_raw(raw: u16) -> Option<Self> {
        match raw {
            0 => Some(Self::Stored),
            8 => Some(Self::Deflated),
            _ => None,
        }
    }

    /// Returns the raw ZIP method id exactly as it appears on disk.
    #[must_use]
    pub fn raw(self) -> u16 {
        match self {
            Self::Stored => 0,
            Self::Deflated => 8,
        }
    }
}

/// Whether the entry payload bytes are stored as-is or need decompression.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataKind {
    /// The payload bytes in [`DataRange::data_range`] are the uncompressed file.
    Stored,
    /// The payload bytes in [`DataRange::data_range`] are compressed with the
    /// reported method.
    Compressed(Compression),
}

/// Absolute archive byte ranges describing an entry's local header and payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DataRange {
    /// Full local-header area, from the local-header signature through the end
    /// of the local extra field.
    pub local_header_range: Range<u64>,
    /// File-name subrange inside [`Self::local_header_range`].
    pub local_name_range: Range<u64>,
    /// Extra-field subrange inside [`Self::local_header_range`].
    pub local_extra_range: Range<u64>,
    /// Payload byte range. The bytes are either stored or compressed depending
    /// on [`Self::kind`].
    pub data_range: Range<u64>,
    /// How to interpret the bytes in [`Self::data_range`].
    pub kind: DataKind,
}

/// Top-level parser error.
#[derive(Debug, Eq, PartialEq)]
pub enum Error<E> {
    /// Error returned by the underlying [`Reader`].
    Io(E),
    /// No EOCD record was found in the allowed scan window.
    NotZip,
    /// A required record or payload extends past the end of the archive.
    Truncated,
    /// A required ZIP signature was missing.
    InvalidSignature,
    /// An archive offset was inconsistent or impossible.
    InvalidOffset,
    /// A record was malformed even though its outer signature matched.
    InvalidRecord,
    /// Split or multi-disk archives.
    MultiDisk,
    /// Strong encryption markers in central or local headers.
    StrongEncryption,
    /// Masked local headers, which hide required local metadata.
    MaskedLocalHeaders,
    /// A compression method the crate does not support.
    UnsupportedCompression(u16),
}

impl<E: fmt::Display> fmt::Display for Error<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "I/O error: {err}"),
            Self::NotZip => f.write_str("not a ZIP archive"),
            Self::Truncated => f.write_str("truncated ZIP archive"),
            Self::InvalidSignature => f.write_str("invalid ZIP signature"),
            Self::InvalidOffset => f.write_str("invalid ZIP offset"),
            Self::InvalidRecord => f.write_str("invalid ZIP record"),
            Self::MultiDisk => f.write_str("multi-disk ZIP archives are unsupported"),
            Self::StrongEncryption => f.write_str("strong encryption is unsupported"),
            Self::MaskedLocalHeaders => f.write_str("masked local headers are unsupported"),
            Self::UnsupportedCompression(method) => {
                write!(f, "unsupported ZIP compression method {method}")
            }
        }
    }
}

/// Open ZIP archive backed by a random-access reader.
///
/// The archive stores only fixed-size metadata. Entry records and local headers
/// are re-read on demand.
pub struct Archive<R> {
    reader: R,
    size: u64,
    base_offset: u64,
    directory_end_offset: u64,
    central_directory_offset: u64,
    entry_count: u64,
}

impl<R: Reader> Archive<R> {
    /// Opens an archive by locating EOCD, resolving ZIP64 metadata, and
    /// validating the central directory layout.
    ///
    /// The parser accepts prefixed archives and trailing junk, but rejects
    /// multi-disk archives and several unsupported encrypted forms.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] for reader failures and a structural [`Error`]
    /// variant when the archive structure is unsupported or malformed.
    pub fn open(reader: R) -> Result<Self, Error<R::Error>> {
        let size = reader.size().map_err(Error::Io)?;
        let (eocd_offset, eocd) = find_eocd(&reader, size)?;

        ensure_single_disk(
            u32::from(eocd.disk_number),
            u32::from(eocd.central_directory_disk),
        )?;

        let (entry_count, central_directory_size, central_directory_offset, payload_end) =
            if eocd.needs_zip64() {
                let zip64 = parse_zip64_metadata(&reader, size, eocd_offset)?;
                ensure_single_disk(zip64.disk_number, zip64.central_directory_disk)?;
                (
                    zip64.entry_count,
                    zip64.central_directory_size,
                    zip64.central_directory_offset,
                    zip64.record_offset,
                )
            } else {
                (
                    u64::from(eocd.entry_count),
                    u64::from(eocd.central_directory_size),
                    u64::from(eocd.central_directory_offset),
                    eocd_offset,
                )
            };

        let used = add(central_directory_offset, central_directory_size)?;
        let inferred_base_offset = payload_end.checked_sub(used).ok_or(Error::InvalidOffset)?;
        let absolute_cd_offset = resolve_central_directory_offset(
            &reader,
            size,
            central_directory_offset,
            inferred_base_offset,
        )?;
        let base_offset = absolute_cd_offset
            .checked_sub(central_directory_offset)
            .ok_or(Error::InvalidOffset)?;
        let absolute_cd_end = add(absolute_cd_offset, central_directory_size)?;
        if absolute_cd_end > eocd_offset || eocd_offset + EOCD_LEN as u64 > size {
            return Err(Error::InvalidOffset);
        }
        if entry_count == 0
            && central_directory_size == 0
            && central_directory_offset == 0
            && eocd_offset != 0
        {
            return Err(Error::InvalidOffset);
        }

        Ok(Self {
            reader,
            size,
            base_offset,
            directory_end_offset: payload_end,
            central_directory_offset,
            entry_count,
        })
    }

    /// Returns the total archive size in bytes.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Returns the number of entries reported by the authoritative central
    /// directory.
    pub fn entry_count(&self) -> u64 {
        self.entry_count
    }

    /// Returns a forward-only iterator over central-directory entries.
    ///
    /// The iterator performs no allocation and stops permanently after the
    /// first parse error.
    pub fn entries(&self) -> Entries<'_, R> {
        Entries {
            archive: self,
            next_offset: self.absolute_central_directory_offset(),
            remaining: self.entry_count,
            end_offset: self.directory_end_offset,
        }
    }

    fn absolute_central_directory_offset(&self) -> u64 {
        self.base_offset + self.central_directory_offset
    }

    fn absolute_local_offset(&self, local_offset: u64) -> Result<u64, Error<R::Error>> {
        add(self.base_offset, local_offset)
    }

    fn read_exact_at(&self, pos: u64, buf: &mut [u8]) -> Result<(), Error<R::Error>> {
        let len = u64::try_from(buf.len()).map_err(|_| Error::InvalidOffset)?;
        if add(pos, len)? > self.size {
            return Err(Error::Truncated);
        }
        self.reader.read_exact_at(pos, buf).map_err(Error::Io)
    }
}

/// Forward-only iterator over archive entries.
pub struct Entries<'a, R> {
    archive: &'a Archive<R>,
    next_offset: u64,
    remaining: u64,
    end_offset: u64,
}

impl<'a, R: Reader> Iterator for Entries<'a, R> {
    type Item = Result<Entry<'a, R>, Error<R::Error>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }

        let result = Entry::parse(self.archive, self.next_offset, self.end_offset);
        match result {
            Ok((entry, next_offset)) => {
                self.remaining -= 1;
                self.next_offset = next_offset;
                Some(Ok(entry))
            }
            Err(err) => {
                self.remaining = 0;
                Some(Err(err))
            }
        }
    }
}

/// Parsed central-directory entry.
///
/// Variable-length fields remain in the underlying archive and can be read into
/// caller-provided buffers with the `read_*` methods.
pub struct Entry<'a, R> {
    archive: &'a Archive<R>,
    name_range: Range<u64>,
    flags: u16,
    compression: Compression,
    crc32: u32,
    compressed_size: u64,
    uncompressed_size: u64,
    local_header_offset: u64,
    _marker: PhantomData<&'a R>,
}

impl<'a, R: Reader> Entry<'a, R> {
    fn parse(
        archive: &'a Archive<R>,
        header_offset: u64,
        end_offset: u64,
    ) -> Result<(Self, u64), Error<R::Error>> {
        let mut header = [0u8; CENTRAL_HEADER_LEN];
        archive.read_exact_at(header_offset, &mut header)?;
        if le_u32(&header[0..4]) != CENTRAL_HEADER_SIGNATURE {
            return Err(Error::InvalidSignature);
        }

        let flags = le_u16(&header[8..10]);
        if flags & (1 << 6) != 0 || flags & (1 << 13) != 0 {
            return Err(Error::StrongEncryption);
        }

        let name_len = u64::from(le_u16(&header[28..30]));
        let extra_len = u64::from(le_u16(&header[30..32]));
        let comment_len = u64::from(le_u16(&header[32..34]));
        let record_len =
            central_record_len(name_len, extra_len, comment_len).ok_or(Error::InvalidOffset)?;
        let next_offset = add(header_offset, record_len)?;
        if next_offset > end_offset {
            return Err(Error::Truncated);
        }

        let name_range = (header_offset + CENTRAL_HEADER_LEN as u64)
            ..(header_offset + CENTRAL_HEADER_LEN as u64 + name_len);
        let extra_range = name_range.end..name_range.end + extra_len;

        let raw_compressed_size = le_u32(&header[20..24]);
        let raw_uncompressed_size = le_u32(&header[24..28]);
        let raw_local_offset = le_u32(&header[42..46]);

        let mut compressed_size = u64::from(raw_compressed_size);
        let mut uncompressed_size = u64::from(raw_uncompressed_size);
        let mut local_header_offset = u64::from(raw_local_offset);

        let zip64_needed = raw_compressed_size == u32::MAX
            || raw_uncompressed_size == u32::MAX
            || raw_local_offset == u32::MAX;
        if zip64_needed {
            let mut scratch = [0u8; 256];
            let extra_len_usize = usize::try_from(extra_len).map_err(|_| Error::InvalidOffset)?;
            if extra_len_usize > scratch.len() {
                return Err(Error::InvalidRecord);
            }
            archive.read_exact_at(extra_range.start, &mut scratch[..extra_len_usize])?;
            let zip64 = find_zip64_extra(
                &scratch[..extra_len_usize],
                raw_uncompressed_size == u32::MAX,
                raw_compressed_size == u32::MAX,
                raw_local_offset == u32::MAX,
            )?;
            if let Some(size) = zip64.uncompressed_size {
                uncompressed_size = size;
            }
            if let Some(size) = zip64.compressed_size {
                compressed_size = size;
            }
            if let Some(offset) = zip64.local_header_offset {
                local_header_offset = offset;
            }
        }

        let compression_method = le_u16(&header[10..12]);
        let compression = Compression::from_raw(compression_method)
            .ok_or(Error::UnsupportedCompression(compression_method))?;

        let entry = Self {
            archive,
            name_range,
            flags,
            compression,
            crc32: le_u32(&header[16..20]),
            compressed_size,
            uncompressed_size,
            local_header_offset,
            _marker: PhantomData,
        };
        Ok((entry, next_offset))
    }

    /// Returns the raw general-purpose bit flags from the central directory.
    #[must_use]
    pub fn flags(&self) -> u16 {
        self.flags
    }

    /// Returns the compression method reported by the central directory.
    #[must_use]
    pub fn compression(&self) -> Compression {
        self.compression
    }

    /// Returns the CRC-32 reported by the central directory.
    #[must_use]
    pub fn crc32(&self) -> u32 {
        self.crc32
    }

    /// Returns the compressed payload size in bytes.
    #[must_use]
    pub fn compressed_size(&self) -> u64 {
        self.compressed_size
    }

    /// Returns the uncompressed payload size in bytes.
    #[must_use]
    pub fn uncompressed_size(&self) -> u64 {
        self.uncompressed_size
    }

    /// Reads the central-directory path bytes into `buf`.
    ///
    /// ZIP stores a single byte string here rather than a structured path. It
    /// may be just a bare file name, a `/`-separated nested path, or a
    /// directory marker ending in `/`. Use [`Self::path_is_utf8`] to check
    /// whether the central-directory metadata declares this path as UTF-8.
    ///
    /// `buf` must be large enough to hold the full stored path. The returned
    /// slice borrows `buf` and is exactly the bytes read from the archive.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidOffset`] if `buf` is too small or the entry range is
    /// inconsistent, and [`Error::Io`] if the underlying read fails.
    pub fn read_path<'b>(&self, buf: &'b mut [u8]) -> Result<&'b [u8], Error<R::Error>> {
        read_variable_range(self.archive, self.name_range.clone(), buf)
    }

    /// Returns whether the central-directory general-purpose bit flag declares
    /// the entry path to be UTF-8 encoded.
    ///
    /// The result comes from bit 11 of the central-directory general-purpose
    /// flag. It does not inspect or validate the path bytes themselves.
    #[must_use]
    pub fn path_is_utf8(&self) -> bool {
        self.flags & (1 << 11) != 0
    }

    /// Returns whether the final `/`-separated component of the entry path
    /// equals `file_name`.
    ///
    /// This compares raw bytes only. It does not decode text, normalize path
    /// separators, or resolve `.` / `..`. The method reads only the path bytes
    /// needed to locate and compare the last component.
    ///
    /// # Errors
    ///
    /// Returns a structural [`Error`] if the stored path range is inconsistent,
    /// and [`Error::Io`] if the underlying reads fail.
    pub fn filename_is(&self, file_name: &[u8]) -> Result<bool, Error<R::Error>> {
        let component_start = find_path_file_name_start(self.archive, self.name_range.clone())?;
        let component_len = self
            .name_range
            .end
            .checked_sub(component_start)
            .ok_or(Error::InvalidOffset)?;
        let file_name_len = u64::try_from(file_name.len()).map_err(|_| Error::InvalidOffset)?;
        if component_len != file_name_len {
            return Ok(false);
        }

        let mut scratch = [0u8; PATH_SCAN_CHUNK_LEN];
        let mut compared = 0usize;
        while compared < file_name.len() {
            let chunk_len = (file_name.len() - compared).min(scratch.len());
            let chunk_offset =
                component_start + u64::try_from(compared).map_err(|_| Error::InvalidOffset)?;
            self.archive
                .read_exact_at(chunk_offset, &mut scratch[..chunk_len])?;
            if scratch[..chunk_len] != file_name[compared..compared + chunk_len] {
                return Ok(false);
            }
            compared += chunk_len;
        }
        Ok(true)
    }

    /// Resolves the local-header and payload ranges for this entry.
    ///
    /// This performs an extra local-header read. It does not decompress data;
    /// it only reports where the stored or compressed bytes live in the
    /// archive.
    ///
    /// # Errors
    ///
    /// Returns a structural [`Error`] if the local header is malformed or uses an
    /// unsupported feature, and [`Error::Io`] if the underlying read fails.
    pub fn data_range(&self) -> Result<DataRange, Error<R::Error>> {
        if self.flags & 1 != 0 {
            return Err(Error::StrongEncryption);
        }

        let local_header_offset = self
            .archive
            .absolute_local_offset(self.local_header_offset)?;
        let mut header = [0u8; LOCAL_HEADER_LEN];
        self.archive
            .read_exact_at(local_header_offset, &mut header)?;
        if le_u32(&header[0..4]) != LOCAL_HEADER_SIGNATURE {
            return Err(Error::InvalidSignature);
        }
        let local_flags = le_u16(&header[6..8]);
        if local_flags & 1 != 0 {
            return Err(Error::StrongEncryption);
        }
        if local_flags & (1 << 13) != 0 {
            return Err(Error::MaskedLocalHeaders);
        }

        let local_name_len = u64::from(le_u16(&header[26..28]));
        let local_extra_len = u64::from(le_u16(&header[28..30]));
        let local_name_range = (local_header_offset + LOCAL_HEADER_LEN as u64)
            ..(local_header_offset + LOCAL_HEADER_LEN as u64 + local_name_len);
        let local_extra_range = local_name_range.end..local_name_range.end + local_extra_len;
        let data_start = local_extra_range.end;
        let data_end = add(data_start, self.compressed_size)?;
        if data_end > self.archive.size() {
            return Err(Error::Truncated);
        }

        Ok(DataRange {
            local_header_range: local_header_offset..data_start,
            local_name_range,
            local_extra_range,
            data_range: data_start..data_end,
            kind: match self.compression {
                Compression::Stored => DataKind::Stored,
                other => DataKind::Compressed(other),
            },
        })
    }
}

fn read_variable_range<'a, R: Reader>(
    archive: &Archive<R>,
    range: Range<u64>,
    buf: &'a mut [u8],
) -> Result<&'a [u8], Error<R::Error>> {
    let len = range_len_usize(&range)?;
    if buf.len() < len {
        return Err(Error::InvalidOffset);
    }
    archive.read_exact_at(range.start, &mut buf[..len])?;
    Ok(&buf[..len])
}

fn find_path_file_name_start<R: Reader>(
    archive: &Archive<R>,
    path_range: Range<u64>,
) -> Result<u64, Error<R::Error>> {
    let mut cursor = path_range.end;
    let mut scratch = [0u8; PATH_SCAN_CHUNK_LEN];

    while cursor > path_range.start {
        let remaining = cursor
            .checked_sub(path_range.start)
            .ok_or(Error::InvalidOffset)?;
        let chunk_len_u64 = remaining.min(PATH_SCAN_CHUNK_LEN as u64);
        let chunk_len = usize::try_from(chunk_len_u64).map_err(|_| Error::InvalidOffset)?;
        let chunk_start = cursor - chunk_len_u64;
        archive.read_exact_at(chunk_start, &mut scratch[..chunk_len])?;

        if let Some(index) = scratch[..chunk_len].iter().rposition(|&byte| byte == b'/') {
            return Ok(chunk_start + index as u64 + 1);
        }

        cursor = chunk_start;
    }

    Ok(path_range.start)
}

#[derive(Clone, Copy)]
struct Eocd {
    disk_number: u16,
    central_directory_disk: u16,
    entry_count: u16,
    central_directory_size: u32,
    central_directory_offset: u32,
}

impl Eocd {
    fn needs_zip64(self) -> bool {
        self.entry_count == u16::MAX
            || self.central_directory_size == u32::MAX
            || self.central_directory_offset == u32::MAX
    }
}

#[derive(Clone, Copy)]
struct Zip64Record {
    record_offset: u64,
    disk_number: u32,
    central_directory_disk: u32,
    entry_count: u64,
    central_directory_size: u64,
    central_directory_offset: u64,
}

#[derive(Clone, Copy, Default)]
struct Zip64Extra {
    uncompressed_size: Option<u64>,
    compressed_size: Option<u64>,
    local_header_offset: Option<u64>,
}

#[allow(clippy::large_stack_arrays)]
fn find_eocd<R: Reader>(reader: &R, size: u64) -> Result<(u64, Eocd), Error<R::Error>> {
    if size < EOCD_LEN as u64 {
        return Err(Error::NotZip);
    }

    let window_u64 = size.min(MAX_EOCD_SCAN as u64);
    let window = usize::try_from(window_u64).map_err(|_| Error::InvalidOffset)?;
    let start = size - window_u64;
    let mut buffer = [0u8; MAX_EOCD_SCAN];
    let buf = &mut buffer[..window];
    reader.read_exact_at(start, buf).map_err(Error::Io)?;

    for idx in (0..=window - EOCD_LEN).rev() {
        if le_u32(&buf[idx..idx + 4]) != EOCD_SIGNATURE {
            continue;
        }
        let comment_len = usize::from(le_u16(&buf[idx + 20..idx + 22]));
        let end = idx + EOCD_LEN + comment_len;
        if end > window {
            continue;
        }
        let eocd = Eocd {
            disk_number: le_u16(&buf[idx + 4..idx + 6]),
            central_directory_disk: le_u16(&buf[idx + 6..idx + 8]),
            entry_count: le_u16(&buf[idx + 10..idx + 12]),
            central_directory_size: le_u32(&buf[idx + 12..idx + 16]),
            central_directory_offset: le_u32(&buf[idx + 16..idx + 20]),
        };
        return Ok((start + idx as u64, eocd));
    }

    Err(Error::NotZip)
}

fn parse_zip64_metadata<R: Reader>(
    reader: &R,
    size: u64,
    eocd_offset: u64,
) -> Result<Zip64Record, Error<R::Error>> {
    let locator_offset = eocd_offset
        .checked_sub(ZIP64_LOCATOR_LEN as u64)
        .ok_or(Error::InvalidRecord)?;
    let mut locator = [0u8; ZIP64_LOCATOR_LEN];
    reader
        .read_exact_at(locator_offset, &mut locator)
        .map_err(Error::Io)?;
    if le_u32(&locator[0..4]) != ZIP64_LOCATOR_SIGNATURE {
        return Err(Error::InvalidSignature);
    }

    let disk_number = le_u32(&locator[4..8]);
    let zip64_offset = le_u64(&locator[8..16]);
    let total_disks = le_u32(&locator[16..20]);
    if disk_number != 0 || total_disks != 1 {
        return Err(Error::MultiDisk);
    }

    let zip64_offset = resolve_zip64_record_offset(reader, size, locator_offset, zip64_offset)?;
    let mut header = [0u8; 56];
    if add(zip64_offset, header.len() as u64)? > size {
        return Err(Error::Truncated);
    }
    reader
        .read_exact_at(zip64_offset, &mut header)
        .map_err(Error::Io)?;
    if le_u32(&header[0..4]) != ZIP64_EOCD_SIGNATURE {
        return Err(Error::InvalidSignature);
    }

    let record_size = le_u64(&header[4..12]);
    let total_len = add(record_size, 12)?;
    if add(zip64_offset, total_len)? > size {
        return Err(Error::Truncated);
    }

    Ok(Zip64Record {
        record_offset: zip64_offset,
        disk_number: le_u32(&header[16..20]),
        central_directory_disk: le_u32(&header[20..24]),
        entry_count: le_u64(&header[32..40]),
        central_directory_size: le_u64(&header[40..48]),
        central_directory_offset: le_u64(&header[48..56]),
    })
}

fn resolve_zip64_record_offset<R: Reader>(
    reader: &R,
    size: u64,
    locator_offset: u64,
    advertised_offset: u64,
) -> Result<u64, Error<R::Error>> {
    const SEARCH_WINDOW: usize = 4096;

    if looks_like_signature(reader, size, advertised_offset, ZIP64_EOCD_SIGNATURE)? {
        return Ok(advertised_offset);
    }

    let start = locator_offset.saturating_sub(SEARCH_WINDOW as u64);
    let window = locator_offset
        .checked_sub(start)
        .ok_or(Error::InvalidOffset)?;
    let window_usize = usize::try_from(window).map_err(|_| Error::InvalidOffset)?;
    let mut buffer = [0u8; SEARCH_WINDOW];
    reader
        .read_exact_at(start, &mut buffer[..window_usize])
        .map_err(Error::Io)?;

    if window_usize < 4 {
        return Err(Error::InvalidSignature);
    }
    for idx in (0..=window_usize - 4).rev() {
        if le_u32(&buffer[idx..idx + 4]) == ZIP64_EOCD_SIGNATURE {
            return Ok(start + idx as u64);
        }
    }

    Err(Error::InvalidSignature)
}

fn resolve_central_directory_offset<R: Reader>(
    reader: &R,
    size: u64,
    central_directory_offset: u64,
    inferred_base_offset: u64,
) -> Result<u64, Error<R::Error>> {
    let raw_offset = central_directory_offset;
    let inferred_offset = add(inferred_base_offset, central_directory_offset)?;

    let raw_valid = looks_like_central_header(reader, size, raw_offset)?;
    let inferred_valid = looks_like_central_header(reader, size, inferred_offset)?;

    if raw_valid {
        return Ok(raw_offset);
    }
    if inferred_valid {
        return Ok(inferred_offset);
    }
    if inferred_offset <= size {
        return Ok(inferred_offset);
    }
    Err(Error::InvalidOffset)
}

fn looks_like_central_header<R: Reader>(
    reader: &R,
    size: u64,
    offset: u64,
) -> Result<bool, Error<R::Error>> {
    looks_like_signature(reader, size, offset, CENTRAL_HEADER_SIGNATURE)
}

fn looks_like_signature<R: Reader>(
    reader: &R,
    size: u64,
    offset: u64,
    signature: u32,
) -> Result<bool, Error<R::Error>> {
    let Some(end) = offset.checked_add(4) else {
        return Ok(false);
    };
    if end > size {
        return Ok(false);
    }

    let mut bytes = [0u8; 4];
    reader
        .read_exact_at(offset, &mut bytes)
        .map_err(Error::Io)?;
    Ok(le_u32(&bytes) == signature)
}

fn find_zip64_extra<E>(
    mut extra: &[u8],
    need_uncompressed: bool,
    need_compressed: bool,
    need_offset: bool,
) -> Result<Zip64Extra, Error<E>> {
    let mut out = Zip64Extra::default();
    while extra.len() >= 4 {
        let kind = le_u16(&extra[0..2]);
        let len = usize::from(le_u16(&extra[2..4]));
        extra = &extra[4..];
        if len > extra.len() {
            return Err(Error::InvalidRecord);
        }
        let field = &extra[..len];
        extra = &extra[len..];

        if kind != 0x0001 {
            continue;
        }

        let mut pos = 0usize;
        if need_uncompressed {
            out.uncompressed_size = Some(read_extra_u64(field, &mut pos)?);
        }
        if need_compressed {
            out.compressed_size = Some(read_extra_u64(field, &mut pos)?);
        }
        if need_offset {
            out.local_header_offset = Some(read_extra_u64(field, &mut pos)?);
        }
        return Ok(out);
    }
    Err(Error::InvalidRecord)
}

fn read_extra_u64<E>(extra: &[u8], pos: &mut usize) -> Result<u64, Error<E>> {
    let end = pos.checked_add(8).ok_or(Error::InvalidOffset)?;
    if end > extra.len() {
        return Err(Error::InvalidRecord);
    }
    let value = le_u64(&extra[*pos..end]);
    *pos = end;
    Ok(value)
}

fn ensure_single_disk<E>(disk_number: u32, central_directory_disk: u32) -> Result<(), Error<E>> {
    if disk_number != 0 || central_directory_disk != 0 {
        return Err(Error::MultiDisk);
    }
    Ok(())
}

fn add<E>(lhs: u64, rhs: u64) -> Result<u64, Error<E>> {
    lhs.checked_add(rhs).ok_or(Error::InvalidOffset)
}

fn central_record_len(name_len: u64, extra_len: u64, comment_len: u64) -> Option<u64> {
    (CENTRAL_HEADER_LEN as u64)
        .checked_add(name_len)
        .and_then(|v| v.checked_add(extra_len))
        .and_then(|v| v.checked_add(comment_len))
}

fn range_len_usize<E>(range: &Range<u64>) -> Result<usize, Error<E>> {
    let len = range
        .end
        .checked_sub(range.start)
        .ok_or(Error::InvalidOffset)?;
    usize::try_from(len).map_err(|_| Error::InvalidOffset)
}

fn le_u16(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[0], bytes[1]])
}

fn le_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn le_u64(bytes: &[u8]) -> u64 {
    u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ])
}
