//! A stored (uncompressed) zip archive, written as AnkiQuest-Art writes its
//! packs (`tools/art-export/src/zip.rs`): entries in name order, the earliest
//! time a zip can hold, mode 0644, no extra fields or comments. The pictures
//! are compressed already.

use std::collections::BTreeMap;

/// Local file header signature.
const LOCAL: u32 = 0x0403_4b50;

/// Central directory header signature.
const CENTRAL: u32 = 0x0201_4b50;

/// End of central directory signature.
const END: u32 = 0x0605_4b50;

/// Version needed to extract: 2.0.
const VERSION: u16 = 20;

/// Version made by: Unix, 2.0, so the mode below is read.
const MADE_BY: u16 = (3 << 8) | VERSION;

/// DOS date of 1980-01-01, with time 00:00:00.
const DATE: u16 = (1 << 5) | 1;

/// Regular file, mode 0644, in the high half of the external attributes.
const MODE: u32 = 0o100_644 << 16;

/// The zip of `files` (name, bytes), stored. Fails no input: names and
/// sizes stay far below the 16- and 32-bit limits for companion packs.
pub fn stored(files: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let offset = out.len() as u32;
        let crc = crc32fast::hash(data);
        let size = data.len() as u32;
        let name_len = name.len() as u16;

        // Local header and data.
        put32(&mut out, LOCAL);
        put16(&mut out, VERSION);
        common(&mut out, crc, size, name_len);
        put16(&mut out, 0);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);

        // Central directory entry pointing at it.
        put32(&mut central, CENTRAL);
        put16(&mut central, MADE_BY);
        put16(&mut central, VERSION);
        common(&mut central, crc, size, name_len);
        put16(&mut central, 0); // extra
        put16(&mut central, 0); // comment
        put16(&mut central, 0); // disk
        put16(&mut central, 0); // internal attributes
        put32(&mut central, MODE);
        put32(&mut central, offset);
        central.extend_from_slice(name.as_bytes());
    }

    // Central directory, then its end record.
    let start = out.len() as u32;
    out.extend_from_slice(&central);
    let count = files.len() as u16;
    put32(&mut out, END);
    put16(&mut out, 0);
    put16(&mut out, 0);
    put16(&mut out, count);
    put16(&mut out, count);
    put32(&mut out, central.len() as u32);
    put32(&mut out, start);
    put16(&mut out, 0);
    out
}

/// The fields local and central headers share, from flags to name length.
fn common(out: &mut Vec<u8>, crc: u32, size: u32, name_len: u16) {
    put16(out, 0); // flags
    put16(out, 0); // stored
    put16(out, 0); // time
    put16(out, DATE);
    put32(out, crc);
    put32(out, size);
    put32(out, size);
    put16(out, name_len);
}

fn put16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zip_reader_reads_what_it_writes() {
        let mut files = BTreeMap::new();
        files.insert("b.webp".to_string(), vec![1, 2, 3]);
        files.insert("a.json".to_string(), b"{}\n".to_vec());
        let bytes = stored(&files);

        // Another implementation checks the headers and CRCs.
        let mut archive = ::zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut seen = Vec::new();
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).unwrap();
            let mut data = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut data).unwrap();
            assert_eq!(entry.compression(), ::zip::CompressionMethod::Stored);
            assert_eq!(entry.unix_mode(), Some(0o100_644));
            seen.push((entry.name().to_string(), data));
        }
        assert_eq!(
            seen,
            [
                ("a.json".to_string(), b"{}\n".to_vec()),
                ("b.webp".to_string(), vec![1, 2, 3])
            ]
        );
    }

    #[test]
    fn same_input_same_bytes() {
        let mut files = BTreeMap::new();
        files.insert("x".to_string(), vec![9; 100]);
        assert_eq!(stored(&files), stored(&files));
    }
}
