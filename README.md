# tinyzip

[![crates.io](https://img.shields.io/crates/v/tinyzip.svg)](https://crates.io/crates/tinyzip)
[![docs.rs](https://img.shields.io/docsrs/tinyzip.svg)](https://docs.rs/tinyzip)

`tinyzip` is a `no_std` low level ZIP navigation library for Rust.
It does not have any dependency and does not allocate memory.

This crate does not decompress data: you iterate
over files in a ZIP archive, and get access to raw bytes.
You can decompress them with an external crate like [miniz_oxide](https://docs.rs/miniz_oxide) or [flate2](https://docs.rs/flate2).

## About the ZIP format

A ZIP archive has the following overall structure:

```text
[local file header 1] [file data 1]
[local file header 2] [file data 2]
...
[central directory header 1]
[central directory header 2]
...
[end of central directory record]
```

### Central directory vs. local headers

Each file's metadata is stored **twice**: once in a _local file header_ immediately
before the file data, and once in the _central directory_ near the end of the archive.
The central directory is the authoritative source. It contains the full metadata and
a pointer (byte offset) to each local header.

**This crate reads the central directory.** It uses local headers only to resolve the
exact byte offset of file data (since the local header contains variable-length fields
that can shift the data start). You should not rely on local header fields directly —
some writers zero them out (e.g. when streaming, using a _data descriptor_ after the
file data instead), and the central-directory-encryption feature intentionally masks them.

### File name and path encoding

File names are stored as raw bytes. The ZIP specification originally required
[IBM Code Page 437](https://en.wikipedia.org/wiki/Code_page_437) encoding, but in
practice many archivers write whatever the local OS encoding is.

If **general purpose bit 11** (the "Language Encoding Flag", EFS) is set, the file name
and comment are guaranteed to be **UTF-8**. You can check this with `Entry::path_is_utf8()`.
This crate gives you the raw bytes; decoding them is left to you.

Path separators are always forward slashes (`/`). Directory entries are indicated by a
trailing `/`. There is no leading slash and no drive letter.

### What the spec does and does not guarantee

- The compression method can be `Stored` (no compression) or `Deflate` (by far the most common). Other values are rare and not supported by this crate.
- File order in the archive is arbitrary.
- For files larger than ~4 GB, **ZIP64** extensions are used. This crate handles ZIP64 transparently.

The full format specification is [APPNOTE.TXT](https://pkware.cachefly.net/webdocs/casestudies/APPNOTE.TXT),
maintained by PKWARE.

## Supported

- Single-disk ZIP and ZIP64 archives
- Leading prefix data and trailing junk
- Central-directory iteration without buffering the directory
- Lazy reading of variable-length metadata and local headers

## Not Supported

- Multi-disk ZIP archives
- Decompression (use the deflate implementation of your choice)
- Filename decoding: you can access the raw bytes and whether the file name is utf8 (it usually is).
- Central-directory encryption or compressed central-directory structures
- Automatic checksum verification (you get access to the checksum if you need it)

## Core API

### no_std

```rust
# fn main() {
#     let file_bytes: &[u8] = include_bytes!("tests/data/manual/go-archive-zip/test.zip");
#     run(file_bytes).unwrap();
# }
# fn run(file_bytes: &[u8]) -> Result<(), tinyzip::Error<tinyzip::SliceReaderError>> {
use tinyzip::{Archive, Compression};
use miniz_oxide::inflate::stream::{inflate, InflateState};
use miniz_oxide::{DataFormat, MZFlush};

let archive = Archive::open(file_bytes)?;
let entry = archive.find_file(b"test.txt")?;
let mut decompressed = [0u8; 1024];
let contents = match entry.compression()? {
    Compression::Deflated => {
        let mut chunks = entry.read_chunks::<512>()?;
        let mut state = InflateState::new(DataFormat::Raw);
        let mut out_pos = 0;
        while let Some(chunk) = chunks.next() {
            let result = inflate(&mut state, chunk?,
                &mut decompressed[out_pos..], MZFlush::None);
            out_pos += result.bytes_written;
        }
        &decompressed[..out_pos]
    }
    Compression::Stored => { entry.read_to_slice(&mut decompressed)? }
};
assert_eq!(contents, b"This is a test text file.\n");
# Ok(())
# }
```

### `std` feature

When `std` is available, this crate unlocks features that require `std` traits or heap allocation.
The core logic remains the same and does not allocate when opening a file or iterating through contents.

```rust,no_run
# #[cfg(feature = "std")]
# fn main() -> Result<(), Box<dyn core::error::Error>> {
use std::fs::File;
use std::io;
use tinyzip::{Archive, Compression};
use flate2::read::DeflateDecoder;

let zip_file = File::open("archive.zip")?;
let archive = Archive::try_from(zip_file)?;
let entry = archive.find_file(b"test.txt")?;
let mut outfile = File::create("test.txt")?;
match entry.compression()? {
    Compression::Deflated => {
        let mut decoder = DeflateDecoder::new(entry.reader()?);
        io::copy(&mut decoder, &mut outfile)?;
    }
    Compression::Stored => {
        io::copy(&mut entry.reader()?, &mut outfile)?;
    }
}
# Ok(())
# }
# #[cfg(not(feature = "std"))]
# fn main() {}
```

## API details

The API stays low-level on purpose:

`Reader` is a tiny random-access trait that can be implemented directly on top
of immutable positioned reads.

Only small fixed-size archive metadata are loaded and stored in memory.
Variable-length fields are read into caller-provided buffers.

Data location is resolved lazily from the local header only when needed.

## Maintenance

pr welcome
