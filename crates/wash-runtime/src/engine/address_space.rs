//! Bounded parsing of Linux's initial stack mapping. Mapping paths are bytes,
//! not necessarily UTF-8, and a filename ending in `[stack]` is not the stack.

use std::io::{self, BufRead, BufReader, Read};

// This is a startup heuristic, not a reason to read an arbitrarily large maps
// file in an embedder that has already created many mappings.
const MAX_MAPS_BYTES: u64 = 16 * 1024 * 1024;
const MAX_MAP_LINE_BYTES: u64 = 64 * 1024;

#[cfg(target_os = "linux")]
pub(super) fn read() -> io::Result<u64> {
    from_reader(std::fs::File::open("/proc/self/maps")?)
}

fn from_reader(reader: impl Read) -> io::Result<u64> {
    let mut reader = BufReader::new(reader.take(MAX_MAPS_BYTES));
    let mut line = Vec::new();
    let mut total = 0;
    loop {
        line.clear();
        let bytes = reader
            .by_ref()
            .take(MAX_MAP_LINE_BYTES + 1)
            .read_until(b'\n', &mut line)?;
        total += bytes as u64;
        if bytes == 0 || total >= MAX_MAPS_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "no initial stack mapping within the maps read limit",
            ));
        }
        if bytes as u64 > MAX_MAP_LINE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "maps line exceeds the read limit",
            ));
        }
        if let Some(space) = from_map(&line) {
            return Ok(space);
        }
    }
}

fn from_map(line: &[u8]) -> Option<u64> {
    let mut fields = line
        .split(u8::is_ascii_whitespace)
        .filter(|field| !field.is_empty());
    let range = fields.next()?;
    let _permissions = fields.next()?;
    let _offset = fields.next()?;
    let device = fields.next()?;
    let inode = fields.next()?;
    if fields.next()? != b"[stack]"
        || fields.next().is_some()
        || device != b"00:00"
        || inode != b"0"
    {
        return None;
    }
    let (start, end) = std::str::from_utf8(range).ok()?.split_once('-')?;
    let start = u64::from_str_radix(start, 16).ok()?;
    let end = u64::from_str_radix(end, 16).ok()?;
    if start >= end {
        return None;
    }
    let space = end.checked_next_power_of_two()?;
    (1 << 32..=1 << 57).contains(&space).then_some(space)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn reads_the_supported_stack_layouts() {
        for (end, bits) in [
            ("7ffd1c3c2000", 47),
            ("ffffc1321000", 48),
            ("7fd5a3f000", 39),
        ] {
            let maps = format!("1000-{end} rw-p 00000000 00:00 0  [stack]\n");
            assert_eq!(from_reader(maps.as_bytes()).unwrap(), 1 << bits);
        }
    }

    #[test]
    fn skips_mapped_filenames_and_non_utf8_paths() {
        let maps = b"100000000-100001000 r--p 00000000 08:01 123 /tmp/[stack]\n\
                     100001000-100002000 r--p 00000000 08:01 124 /tmp/\xff\n\
                     100002000-100003000 r--p 00000000 08:01 125 /tmp/name [stack]\n\
                     7ffd1c3a1000-7ffd1c3c2000 rw-p 00000000 00:00 0 [stack]\n";
        assert_eq!(from_reader(&maps[..]).unwrap(), 1 << 47);
    }

    #[test]
    fn rejects_invalid_ranges_and_non_stack_mappings() {
        for line in [
            "1000-2000 rw-p 0 00:00 0 [stack]",
            "200000000000000-200000000000001 rw-p 0 00:00 0 [stack]",
            "1000-ffffffffffffffff rw-p 0 00:00 0 [stack]",
            "not-a-range rw-p 0 00:00 0 [stack]",
            "ffffc1321000-1000 rw-p 0 00:00 0 [stack]",
            "ffffc1321000-ffffc1321000 rw-p 0 00:00 0 [stack]",
            "1000-ffffc1321000 rw-p 0 00:00 1 [stack]",
            "1000-ffffc1321000 rw-p 0 08:01 0 [stack]",
            "1000-ffffc1321000 rw-p 0 00:00 0 [stack] extra",
            "1000-ffffc1321000 rw-p 0 00:00 0 [anon:stack]",
        ] {
            assert_eq!(from_map(line.as_bytes()), None, "{line}");
        }
        assert!(from_reader(&b"no stack here\n"[..]).is_err());
    }

    #[test]
    fn bounds_both_the_line_and_the_total_read() {
        assert!(from_reader(io::repeat(b'x')).is_err());
        let maps = b"1000-2000 rw-p 00000000 00:00 0\n".repeat(MAX_MAPS_BYTES as usize / 30 + 1);
        let mut cursor = io::Cursor::new(maps);
        assert!(from_reader(&mut cursor).is_err());
        assert!(cursor.position() <= MAX_MAPS_BYTES);
    }

    #[test]
    fn propagates_read_errors() {
        struct Denied;
        impl Read for Denied {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::ErrorKind::PermissionDenied.into())
            }
        }
        assert_eq!(
            from_reader(Denied).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn reads_the_current_process() {
        assert!(read().is_ok());
    }
}
