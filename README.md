# tinyzip

`tinyzip` is a tiny `no_std`, `no_alloc` ZIP navigation crate.

It does not decompress data. It only finds the central directory, iterates
entries, and exposes raw byte ranges so callers can read names, comments, extra
fields, local headers, and the stored or compressed payload bytes themselves.

## Supported

- Single-disk ZIP archives
- Leading prefix data and trailing junk
- EOCD and ZIP64 EOCD/locator
- Central-directory iteration without buffering the directory
- Lazy reading of variable-length metadata and local headers

## Not Supported

- Multi-disk ZIP archives
- Decompression
- Filename decoding / path normalization
- Central-directory encryption or compressed central-directory structures

## Core API

```rust
use tinyzip::{Archive, Reader};

fn inspect<R: Reader>(reader: R) -> Result<(), tinyzip::Error<R::Error>> {
    let archive = Archive::open(reader)?;
    for entry in archive.entries() {
        let entry = entry?;
        let mut name = [0u8; 64];
        let path = entry.read_path(&mut name)?;
        let data = entry.data_range()?;
        let _ = (path, data);
    }
    Ok(())
}
```

The API stays low-level on purpose:

- `Reader` is a tiny random-access trait that can be implemented directly on top
  of immutable positioned reads.
- With the `std` feature, `tinyzip::std_io::ReadSeekReader` adapts `Read + Seek`
  and `tinyzip::std_io::UnixFileReader` adapts Unix `FileExt`.
- `Archive` stores only fixed-size archive metadata.
- `Entry` borrows the archive and holds one parsed central-directory record.
- ZIP paths are exposed as raw bytes; they may be nested `/`-separated paths,
  bare file names, or directory markers ending in `/`.
- Variable-length fields are read into caller-provided buffers.
- Data location is resolved lazily from the local header only when needed.
