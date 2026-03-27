use std::fs;
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::vec::Vec;

use tinyzip::{
    Archive, Compression, DataKind, Error, FormatError, Reader, SliceReaderError, Unsupported,
};

const VALID_ROOT: &str = "tests/data/valid";
const INVALID_ROOT: &str = "tests/data/invalid";

struct CountingReader<'a> {
    bytes: &'a [u8],
    bytes_read: &'a Cell<u64>,
}

impl<'a> CountingReader<'a> {
    fn new(bytes: &'a [u8], bytes_read: &'a Cell<u64>) -> Self {
        Self {
            bytes,
            bytes_read,
        }
    }
}

impl Reader for CountingReader<'_> {
    type Error = ();

    fn size(&self) -> Result<u64, Self::Error> {
        Ok(self.bytes.len() as u64)
    }

    fn read_exact_at(&self, pos: u64, buf: &mut [u8]) -> Result<(), Self::Error> {
        let pos = usize::try_from(pos).map_err(|_| ())?;
        let end = pos.checked_add(buf.len()).ok_or(())?;
        let src = self.bytes.get(pos..end).ok_or(())?;
        buf.copy_from_slice(src);
        self.bytes_read
            .set(self.bytes_read.get() + u64::try_from(buf.len()).map_err(|_| ())?);
        Ok(())
    }
}

struct Fixture {
    name: &'static str,
    path: &'static str,
    expected: Expected,
}

enum Expected {
    Reject(FormatError),
    Accept {
        entry_count: u64,
        entries: &'static [ExpectedEntry],
    },
}

struct ExpectedEntry {
    name: &'static [u8],
    compression: Compression,
    compressed_size: u64,
    uncompressed_size: u64,
}

const TEST_ENTRIES: &[ExpectedEntry] = &[
    ExpectedEntry {
        name: b"test.txt",
        compression: Compression::Deflated,
        compressed_size: 25,
        uncompressed_size: 26,
    },
    ExpectedEntry {
        name: b"gophercolor16x16.png",
        compression: Compression::Stored,
        compressed_size: 785,
        uncompressed_size: 785,
    },
];

const DD_ENTRIES: &[ExpectedEntry] = &[ExpectedEntry {
    name: b"filename",
    compression: Compression::Deflated,
    compressed_size: 24,
    uncompressed_size: 25,
}];

const GO_STORED_ENTRIES: &[ExpectedEntry] = &[
    ExpectedEntry {
        name: b"foo.txt",
        compression: Compression::Stored,
        compressed_size: 4,
        uncompressed_size: 4,
    },
    ExpectedEntry {
        name: b"bar.txt",
        compression: Compression::Stored,
        compressed_size: 4,
        uncompressed_size: 4,
    },
];

const ZIP64_ENTRIES: &[ExpectedEntry] = &[ExpectedEntry {
    name: b"README",
    compression: Compression::Deflated,
    compressed_size: 36,
    uncompressed_size: 36,
}];

const MANUAL_FIXTURES: &[Fixture] = &[
    Fixture {
        name: "test.zip",
        path: "tests/data/manual/go-archive-zip/test.zip",
        expected: Expected::Accept {
            entry_count: 2,
            entries: TEST_ENTRIES,
        },
    },
    Fixture {
        name: "test-trailing-junk.zip",
        path: "tests/data/manual/go-archive-zip/test-trailing-junk.zip",
        expected: Expected::Accept {
            entry_count: 2,
            entries: TEST_ENTRIES,
        },
    },
    Fixture {
        name: "test-prefix.zip",
        path: "tests/data/manual/go-archive-zip/test-prefix.zip",
        expected: Expected::Accept {
            entry_count: 2,
            entries: TEST_ENTRIES,
        },
    },
    Fixture {
        name: "dd.zip",
        path: "tests/data/manual/go-archive-zip/dd.zip",
        expected: Expected::Accept {
            entry_count: 1,
            entries: DD_ENTRIES,
        },
    },
    Fixture {
        name: "go-with-datadesc-sig.zip",
        path: "tests/data/manual/go-archive-zip/go-with-datadesc-sig.zip",
        expected: Expected::Accept {
            entry_count: 2,
            entries: GO_STORED_ENTRIES,
        },
    },
    Fixture {
        name: "zip64.zip",
        path: "tests/data/manual/go-archive-zip/zip64.zip",
        expected: Expected::Accept {
            entry_count: 1,
            entries: ZIP64_ENTRIES,
        },
    },
    Fixture {
        name: "zip64-2.zip",
        path: "tests/data/manual/go-archive-zip/zip64-2.zip",
        expected: Expected::Accept {
            entry_count: 1,
            entries: ZIP64_ENTRIES,
        },
    },
    Fixture {
        name: "readme.notzip",
        path: "tests/data/manual/go-archive-zip/readme.notzip",
        expected: Expected::Reject(FormatError::NotZip),
    },
    Fixture {
        name: "test-baddirsz.zip",
        path: "tests/data/manual/go-archive-zip/test-baddirsz.zip",
        expected: Expected::Accept {
            entry_count: 2,
            entries: TEST_ENTRIES,
        },
    },
    Fixture {
        name: "test-badbase.zip",
        path: "tests/data/manual/go-archive-zip/test-badbase.zip",
        expected: Expected::Accept {
            entry_count: 2,
            entries: TEST_ENTRIES,
        },
    },
];

#[test]
fn manual_inspected_corpus() {
    for fixture in MANUAL_FIXTURES {
        match &fixture.expected {
            Expected::Reject(kind) => {
                let err = open_fixture_expect_err(fixture.path);
                assert_eq!(
                    format_err(&err),
                    *kind,
                    "fixture {}: expected open error {:?}, got {:?}",
                    fixture.name,
                    kind,
                    err
                );
            }
            Expected::Accept { entry_count, entries } => {
                let bytes = read_fixture(fixture.path);
                let archive = open_archive(&bytes, fixture.path);
                assert_archive_meta(fixture.name, &archive, *entry_count);

                let mut iter = archive.entries();
                for (entry_index, expected) in entries.iter().enumerate() {
                    let entry_index = entry_index as u64;
                    let entry = next_entry(fixture.name, &mut iter, entry_index);
                    assert_entry_matches(fixture.name, &archive, entry_index, &entry, expected);
                    let data = entry_data_range(fixture.name, entry_index, &entry);
                    assert_coherent_data_range(fixture.name, &archive, entry_index, &entry, &data);
                }
                assert!(
                    iter.next().is_none(),
                    "fixture {}: expected exactly {} entries",
                    fixture.name,
                    entries.len()
                );
            }
        }
    }
}

#[test]
fn valid_smoke_corpus() {
    let paths = collect_corpus_files(VALID_ROOT);
    assert!(
        !paths.is_empty(),
        "expected at least one file in {VALID_ROOT}"
    );

    for path in paths {
        let bytes = read_fixture(&path);
        let display = path.display().to_string();
        let archive = open_archive(&bytes, &display);
        let mut iter = archive.entries();
        let mut seen = 0u64;

        while seen < archive.entry_count() {
            let entry = next_entry(&display, &mut iter, seen);
            let name = entry_name(&display, seen, &entry);
            let data = entry_data_range(&display, seen, &entry);
            assert_coherent_data_range(&display, &archive, seen, &entry, &data);
            assert_eq!(
                range_len(&data.data_range),
                entry.compressed_size(),
                "fixture {} entry {} (name {}): compressed size mismatch between entry metadata and data range",
                display,
                seen,
                format_name(&name)
            );
            seen += 1;
        }

        if let Some(extra) = iter.next() {
            match extra {
                Ok(entry) => {
                    let name = entry_name(&display, seen, &entry);
                    panic!(
                        "fixture {}: iterator yielded more entries than archive.entry_count()={}, extra entry name={}",
                        display,
                        archive.entry_count(),
                        format_name(&name)
                    );
                }
                Err(err) => {
                    panic!(
                        "fixture {display}: iterator returned error after {seen} successful entries: {err:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn invalid_smoke_corpus() {
    let paths = collect_corpus_files(INVALID_ROOT);
    assert!(
        !paths.is_empty(),
        "expected at least one file in {INVALID_ROOT}"
    );

    for path in paths {
        let bytes = read_fixture(&path);
        let display = path.display().to_string();

        let Some(reason) = fail_reason(&bytes, &display) else {
            panic!(
                "fixture {display}: expected an error at open, iteration, name read, or data-range resolution, but parsing succeeded cleanly"
            );
        };
        assert!(
            !reason.is_empty(),
            "fixture {display}: expected a descriptive failure reason"
        );
    }
}

#[test]
fn synthetic_empty_zip() {
    let bytes = empty_zip();
    let archive = Archive::open(bytes.as_slice()).unwrap();
    assert_eq!(archive.entry_count(), 0);
    assert_eq!(archive.entries().count(), 0);
}

#[test]
fn synthetic_multidisk_is_rejected() {
    let mut bytes = empty_zip();
    bytes[4] = 1;
    let err = Archive::open(bytes.as_slice()).err().unwrap();
    assert_eq!(
        format_err(&err),
        FormatError::Unsupported(Unsupported::MultiDisk)
    );
}

#[test]
fn synthetic_truncated_eocd_is_rejected() {
    let mut bytes = empty_zip();
    bytes.pop();
    let err = Archive::open(bytes.as_slice()).err().unwrap();
    assert_eq!(format_err(&err), FormatError::NotZip);
}

#[test]
fn synthetic_encrypted_entry_is_rejected_on_data_access() {
    let mut bytes = simple_stored_zip(b"secret.txt", b"hidden");
    bytes[6] |= 0x01;
    let archive = Archive::open(bytes.as_slice()).unwrap();
    let entry = archive.entries().next().unwrap().unwrap();
    let err = entry.data_range().unwrap_err();
    assert_eq!(
        format_err(&err),
        FormatError::Unsupported(Unsupported::StrongEncryption)
    );
}

#[test]
fn synthetic_truncated_local_header_is_rejected_on_data_access() {
    let mut bytes = simple_stored_zip(b"name.txt", b"payload");
    bytes[28] = 0xFF;
    bytes[29] = 0xFF;
    let archive = Archive::open(bytes.as_slice()).unwrap();
    let entry = archive.entries().next().unwrap().unwrap();
    let err = entry.data_range().unwrap_err();
    assert_eq!(format_err(&err), FormatError::Truncated);
}

#[test]
fn synthetic_filename_is_reads_only_needed_suffix_bytes() {
    let mut path = vec![b'a'; 512];
    path.extend_from_slice(b"/target.bin");
    let bytes = simple_stored_zip(&path, b"payload");
    let bytes_read = Cell::new(0);
    let reader = CountingReader::new(&bytes, &bytes_read);
    let archive = Archive::open(reader).unwrap();
    let entry = archive.entries().next().unwrap().unwrap();
    let before = bytes_read.get();
    assert!(entry.filename_is(b"target.bin").unwrap());
    let after = bytes_read.get();

    assert!(
        after > before,
        "expected filename_is to perform additional reads beyond archive open"
    );
    assert!(
        after - before < 128,
        "expected filename_is to avoid reading the full path, but it read {} extra bytes for a {}-byte path",
        after - before,
        path.len()
    );
}

fn fail_reason(bytes: &[u8], path: &str) -> Option<String> {
    let archive = match Archive::open(bytes) {
        Ok(archive) => archive,
        Err(err) => return Some(format!("open failed: {err:?}")),
    };

    let mut iter = archive.entries();
    let mut entry_index = 0u64;
    while entry_index < archive.entry_count() {
        let entry = match iter.next() {
            Some(Ok(entry)) => entry,
            Some(Err(err)) => return Some(format!("entry {entry_index} parse failed: {err:?}")),
            None => return Some(format!("iterator ended early after {entry_index} entries")),
        };

        if let Err(err) = entry.read_path(&mut vec![0u8; PATH_BUF_LEN]) {
            return Some(format!("entry {entry_index} name read failed: {err:?}"));
        }
        if let Err(err) = entry.data_range() {
            return Some(format!("entry {entry_index} data range failed: {err:?}"));
        }
        entry_index += 1;
    }

    if let Some(extra) = iter.next() {
        return Some(match extra {
            Ok(_) => "iterator produced extra entry beyond entry_count".to_string(),
            Err(err) => format!("iterator error after {entry_index} successful entries: {err:?}"),
        });
    }

    let _ = path;
    None
}

fn collect_corpus_files(root: &str) -> Vec<PathBuf> {
    let mut paths = fs::read_dir(root)
        .unwrap_or_else(|err| panic!("failed to read corpus dir {root}: {err}"))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

fn read_fixture(path: impl AsRef<Path>) -> Vec<u8> {
    let path = path.as_ref();
    fs::read(path).unwrap_or_else(|err| panic!("failed to read {}: {}", path.display(), err))
}

fn open_fixture_expect_err(path: &str) -> Error<SliceReaderError> {
    let bytes = read_fixture(path);
    match Archive::open(bytes.as_slice()) {
        Ok(archive) => panic!(
            "fixture {}: expected open error, got archive with entry_count={}",
            path,
            archive.entry_count()
        ),
        Err(err) => err,
    }
}

fn open_archive<'a>(bytes: &'a [u8], label: &str) -> Archive<&'a [u8]> {
    Archive::open(bytes)
        .unwrap_or_else(|err| panic!("fixture {label}: expected archive to open, got {err:?}"))
}

fn assert_archive_meta(
    label: &str,
    archive: &Archive<&[u8]>,
    expected_entry_count: u64,
) {
    assert_eq!(
        archive.entry_count(),
        expected_entry_count,
        "fixture {}: entry_count mismatch, expected {}, got {}",
        label,
        expected_entry_count,
        archive.entry_count()
    );
}

fn next_entry<'a>(
    label: &str,
    iter: &mut impl Iterator<Item = Result<tinyzip::Entry<'a, &'a [u8]>, Error<SliceReaderError>>>,
    entry_index: u64,
) -> tinyzip::Entry<'a, &'a [u8]> {
    match iter.next() {
        Some(Ok(entry)) => entry,
        Some(Err(err)) => panic!(
            "fixture {label} entry {entry_index}: expected entry, got iterator error {err:?}"
        ),
        None => panic!(
            "fixture {label} entry {entry_index}: expected entry, iterator ended early"
        ),
    }
}

fn assert_entry_matches(
    label: &str,
    archive: &Archive<&[u8]>,
    entry_index: u64,
    entry: &tinyzip::Entry<'_, &[u8]>,
    expected: &ExpectedEntry,
) {
    let name = entry_name(label, entry_index, entry);
    assert_eq!(
        name.as_slice(),
        expected.name,
        "fixture {} entry {}: name mismatch, expected {}, got {}",
        label,
        entry_index,
        format_name(expected.name),
        format_name(&name)
    );
    assert_eq!(
        entry.compression(),
        expected.compression,
        "fixture {} entry {} (name {}): compression mismatch, expected {:?}, got {:?}",
        label,
        entry_index,
        format_name(&name),
        expected.compression,
        entry.compression()
    );
    assert_eq!(
        entry.compressed_size(),
        expected.compressed_size,
        "fixture {} entry {} (name {}): compressed_size mismatch, expected {}, got {}",
        label,
        entry_index,
        format_name(&name),
        expected.compressed_size,
        entry.compressed_size()
    );
    assert_eq!(
        entry.uncompressed_size(),
        expected.uncompressed_size,
        "fixture {} entry {} (name {}): uncompressed_size mismatch, expected {}, got {}",
        label,
        entry_index,
        format_name(&name),
        expected.uncompressed_size,
        entry.uncompressed_size()
    );
    let _ = archive;
}

fn assert_coherent_data_range(
    label: &str,
    archive: &Archive<&[u8]>,
    entry_index: u64,
    entry: &tinyzip::Entry<'_, &[u8]>,
    data: &tinyzip::DataRange,
) {
    let name = entry_name(label, entry_index, entry);
    let expected_kind = match entry.compression() {
        Compression::Stored => DataKind::Stored,
        other => DataKind::Compressed(other),
    };
    assert_eq!(
        data.kind,
        expected_kind,
        "fixture {} entry {} (name {}): expected data kind {:?}, got {:?}",
        label,
        entry_index,
        format_name(&name),
        expected_kind,
        data.kind
    );
    assert!(
        data.local_header_range.start < data.local_header_range.end,
        "fixture {} entry {} (name {}): invalid local header range {:?}",
        label,
        entry_index,
        format_name(&name),
        data.local_header_range
    );
    assert!(
        data.local_name_range.start >= data.local_header_range.start
            && data.local_name_range.end <= data.local_header_range.end,
        "fixture {} entry {} (name {}): local name range {:?} escapes local header range {:?}",
        label,
        entry_index,
        format_name(&name),
        data.local_name_range,
        data.local_header_range
    );
    assert!(
        data.local_extra_range.start >= data.local_name_range.end
            && data.local_extra_range.end <= data.local_header_range.end,
        "fixture {} entry {} (name {}): local extra range {:?} escapes local header range {:?}",
        label,
        entry_index,
        format_name(&name),
        data.local_extra_range,
        data.local_header_range
    );
    assert!(
        data.data_range.start >= data.local_header_range.end,
        "fixture {} entry {} (name {}): data range {:?} starts before local header ends {:?}",
        label,
        entry_index,
        format_name(&name),
        data.data_range,
        data.local_header_range
    );
    assert!(
        data.data_range.end <= archive.size(),
        "fixture {} entry {} (name {}): data range {:?} exceeds archive size {}",
        label,
        entry_index,
        format_name(&name),
        data.data_range,
        archive.size()
    );
}

fn entry_name(
    label: &str,
    entry_index: u64,
    entry: &tinyzip::Entry<'_, &[u8]>,
) -> Vec<u8> {
    let mut name_buf = vec![0u8; PATH_BUF_LEN];
    match entry.read_path(&mut name_buf) {
        Ok(name) => name.to_vec(),
        Err(err) => panic!(
            "fixture {label} entry {entry_index}: failed to read path bytes: {err:?}"
        ),
    }
}

fn entry_data_range(
    label: &str,
    entry_index: u64,
    entry: &tinyzip::Entry<'_, &[u8]>,
) -> tinyzip::DataRange {
    match entry.data_range() {
        Ok(data) => data,
        Err(err) => panic!(
            "fixture {} entry {}: failed to resolve data range for name {} compression={:?} compressed_size={} uncompressed_size={}: {:?}",
            label,
            entry_index,
            format_name(&entry_name(label, entry_index, entry)),
            entry.compression(),
            entry.compressed_size(),
            entry.uncompressed_size(),
            err
        ),
    }
}

fn format_err<E>(err: &Error<E>) -> FormatError {
    match err {
        Error::Io(_) => panic!("unexpected I/O error"),
        Error::Format(kind) => *kind,
    }
}

fn format_name(bytes: &[u8]) -> String {
    let rendered = String::from_utf8_lossy(bytes);
    if rendered.as_bytes() == bytes {
        format!("{rendered:?}")
    } else {
        format!("{rendered:?} (raw bytes: {bytes:?})")
    }
}

fn empty_zip() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0x0605_4B50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

fn simple_stored_zip(name: &[u8], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let local_offset = 0u32;
    out.extend_from_slice(&0x0403_4B50u32.to_le_bytes());
    out.extend_from_slice(&20u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&to_u32(data.len()).to_le_bytes());
    out.extend_from_slice(&to_u32(data.len()).to_le_bytes());
    out.extend_from_slice(&to_u16(name.len()).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(name);
    out.extend_from_slice(data);

    let cd_offset = out.len();
    out.extend_from_slice(&0x0201_4B50u32.to_le_bytes());
    out.extend_from_slice(&20u16.to_le_bytes());
    out.extend_from_slice(&20u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&to_u32(data.len()).to_le_bytes());
    out.extend_from_slice(&to_u32(data.len()).to_le_bytes());
    out.extend_from_slice(&to_u16(name.len()).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&local_offset.to_le_bytes());
    out.extend_from_slice(name);

    let cd_size = out.len() - cd_offset;
    out.extend_from_slice(&0x0605_4B50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&to_u32(cd_size).to_le_bytes());
    out.extend_from_slice(&to_u32(cd_offset).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

fn range_len(range: &std::ops::Range<u64>) -> u64 {
    range.end - range.start
}

fn to_u16(value: usize) -> u16 {
    u16::try_from(value).expect("value does not fit u16")
}

fn to_u32(value: usize) -> u32 {
    u32::try_from(value).expect("value does not fit u32")
}

const PATH_BUF_LEN: usize = u16::MAX as usize;
