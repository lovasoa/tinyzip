# tinyzip

`tinyzip` is a `no_std` low level ZIP navigation library for Rust.
It does not have any dependency and does not allocate memory.

This crate does not decompress data: you iterate
over files in a ZIP archive, and get access to raw bytes.
You can decompress them with an external crate like [miniz_oxide](https://docs.rs/miniz_oxide) or [flate2](https://docs.rs/flate2).

## About the ZIP format

A ZIP archive has the following overall structure:

```
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
use tinyzip::{Archive, Reader};
use miniz_oxide::decompress_slice_iter_to_slice;

let mut files = Archive::from(file_bytes)?.files();
while let Ok(file) = files.next() {
    if file.path_is("test.txt") {
        let mut decompressed = [0u8; MAX_DECOMPRESSED_SIZE];
        match file.compression()? {
            Deflated => {
                let mut chunks = file.read_chunks::<512>();
                decompress_slice_iter_to_slice(&mut decompressed, chunks, false, true)?;
            }
            Stored => { file.read_to_slice(&mut decompressed)?; }
        }
    }
}
```

### `std` feature

When std is available this crate gives access to some features that require std traits or memory allocation.
The core logic remains the same and does not allocate when opening a file or iterating through contents.

```rust
let zip_file = File::open("archive.zip")?;
let inner_file = Archive::from(zip_file)?.find_file("test.txt");
let mut outfile = File::open("test.txt");
match inner_file.compression()? {
    Deflate => {
        let mut deflater = DeflateDecoder::new(outfile);
        inner_file.write_to(&mut deflater)?;
        deflater.finish()?
    }
    Stored => {
        inner_file.write_to(outfile)?;
    }
}
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
