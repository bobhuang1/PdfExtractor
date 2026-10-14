//! Compound File Binary (CFB / OLE2) reader.
//!
//! A legacy `.doc` is an OLE compound document: a FAT-like container that keeps
//! named streams (`WordDocument`, `1Table`, ...) in fixed-size sectors. This
//! module implements only what is needed to open such a file and read a stream
//! by name, following MS-CFB.
//!
//! Two sizes matter: full sectors (512 or 4096 bytes) hold large streams, while
//! streams shorter than the header's "mini stream cutoff" live inside the root
//! entry's mini stream, addressed in 64-byte mini sectors. Both are handled, so
//! a small `1Table` is not silently read as garbage.

use anyhow::{bail, Context, Result};

/// `D0 CF 11 E0 A1 B1 1A E1`, the compound-file magic.
const SIGNATURE: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
/// The header is one 512-byte sector even when sectors are 4096 bytes.
const HEADER_LEN: usize = 512;
const DIR_ENTRY_LEN: usize = 128;
const FREE: u32 = 0xFFFF_FFFF;
const END_OF_CHAIN: u32 = 0xFFFF_FFFE;
/// Anything at or above this is a reserved sector marker, not an index.
const FIRST_RESERVED: u32 = 0xFFFF_FFFA;
/// A chain longer than this cannot be real; it means the FAT is corrupt.
const MAX_CHAIN: usize = 1 << 20;

fn u16_at(data: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        data.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn u32_at(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        data.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

/// The bytes of full sector `n`, or `None` when it lies outside the file.
fn sector(data: &[u8], sector_size: usize, n: u32) -> Option<&[u8]> {
    let start = HEADER_LEN.checked_add((n as usize).checked_mul(sector_size)?)?;
    let end = start.checked_add(sector_size)?;
    data.get(start..end)
}

/// One directory entry: a named stream or storage.
struct Entry {
    name: String,
    /// 1 = storage, 2 = stream, 5 = root; 0 = unused.
    kind: u8,
    start: u32,
    size: u64,
}

pub struct CompoundFile<'a> {
    data: &'a [u8],
    sector_size: usize,
    mini_sector_size: usize,
    mini_cutoff: u32,
    fat: Vec<u32>,
    mini_fat: Vec<u32>,
    entries: Vec<Entry>,
    mini_stream: Vec<u8>,
}

impl<'a> CompoundFile<'a> {
    /// Parse the container header and index without reading stream contents.
    pub fn open(data: &'a [u8]) -> Result<Self> {
        if data.len() < HEADER_LEN || data.get(..8) != Some(&SIGNATURE[..]) {
            bail!("not an OLE compound file (missing signature)");
        }
        let shift = u16_at(data, 30).context("truncated header")?;
        if !(7..=14).contains(&shift) {
            bail!("unsupported sector shift {shift}");
        }
        let sector_size = 1usize << shift;
        let mini_shift = u16_at(data, 32).unwrap_or(6);
        let mini_sector_size = 1usize << mini_shift.min(12);
        let mini_cutoff = u32_at(data, 56).unwrap_or(4096);
        let major = u16_at(data, 26).unwrap_or(3);

        // The DIFAT lists the sectors that hold the FAT. The header holds the
        // first 109 entries; any more live in a chain of DIFAT sectors.
        let mut difat: Vec<u32> = (0..109).filter_map(|i| u32_at(data, 76 + 4 * i)).collect();
        let mut difat_sector = u32_at(data, 68).unwrap_or(END_OF_CHAIN);
        let mut guard = 0usize;
        while difat_sector < FIRST_RESERVED {
            let Some(blk) = sector(data, sector_size, difat_sector) else {
                break;
            };
            for i in 0..(sector_size / 4).saturating_sub(1) {
                if let Some(v) = u32_at(blk, 4 * i) {
                    difat.push(v);
                }
            }
            difat_sector = u32_at(blk, sector_size - 4).unwrap_or(END_OF_CHAIN);
            guard += 1;
            if guard > MAX_CHAIN {
                bail!("runaway DIFAT chain");
            }
        }

        // Concatenate the FAT sectors into one flat table.
        let declared = u32_at(data, 44).unwrap_or(0) as usize;
        let mut fat = Vec::new();
        for fs in difat {
            if fs >= FIRST_RESERVED {
                continue;
            }
            let Some(blk) = sector(data, sector_size, fs) else {
                continue;
            };
            for i in 0..sector_size / 4 {
                fat.push(u32_at(blk, 4 * i).unwrap_or(FREE));
            }
        }
        if fat.is_empty() {
            bail!("compound file has no FAT sectors");
        }
        // `declared` is a hint only; a shorter table is tolerated, a longer one
        // would index past real data.
        let _ = declared;

        // The count in the header is *sectors*, not entries: the chain must be
        // read to the end, or later streams fall off the truncated table.
        let mini_fat_bytes = read_chain(
            data,
            sector_size,
            &fat,
            u32_at(data, 60).unwrap_or(END_OF_CHAIN),
            u32_at(data, 64).unwrap_or(0) as usize * sector_size,
        );
        let mini_fat: Vec<u32> = mini_fat_bytes
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();

        let directory = read_chain(
            data,
            sector_size,
            &fat,
            u32_at(data, 48).unwrap_or(END_OF_CHAIN),
            usize::MAX,
        );
        let entries = parse_directory(&directory, major == 3);
        if entries.is_empty() {
            bail!("compound file has an empty directory");
        }

        // The root entry (index 0) owns the mini stream, held in full sectors.
        let root = entries.first().expect("checked non-empty");
        let mini_stream = read_chain(data, sector_size, &fat, root.start, root.size as usize);

        Ok(Self {
            data,
            sector_size,
            mini_sector_size,
            mini_cutoff,
            fat,
            mini_fat,
            entries,
            mini_stream,
        })
    }

    /// Every named entry, for diagnostics.
    pub fn entry_names(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|e| e.kind != 0 && !e.name.is_empty())
            .map(|e| e.name.as_str())
            .collect()
    }

    /// The bytes of a named stream, or `None` when it is absent.
    pub fn stream(&self, name: &str) -> Option<Vec<u8>> {
        let entry = self
            .entries
            .iter()
            .find(|e| e.kind == 2 && e.name == name)?;
        Some(self.read_stream_bytes(entry))
    }

    fn read_stream_bytes(&self, entry: &Entry) -> Vec<u8> {
        if entry.size == 0 {
            return Vec::new();
        }
        if entry.size < self.mini_cutoff as u64 {
            self.read_mini(entry.start, entry.size)
        } else {
            read_chain(
                self.data,
                self.sector_size,
                &self.fat,
                entry.start,
                entry.size as usize,
            )
        }
    }

    fn read_mini(&self, start: u32, size: u64) -> Vec<u8> {
        let mut out = Vec::new();
        let mut current = start;
        let mut guard = 0usize;
        while current < FIRST_RESERVED {
            let offset = current as usize * self.mini_sector_size;
            if offset >= self.mini_stream.len() {
                break;
            }
            let end = (offset + self.mini_sector_size).min(self.mini_stream.len());
            out.extend_from_slice(&self.mini_stream[offset..end]);
            if out.len() as u64 >= size {
                break;
            }
            current = *self.mini_fat.get(current as usize).unwrap_or(&END_OF_CHAIN);
            guard += 1;
            if guard > MAX_CHAIN {
                break;
            }
        }
        out.truncate(size as usize);
        out
    }
}

/// Follow a sector chain and return at most `size` bytes.
fn read_chain(data: &[u8], sector_size: usize, fat: &[u32], start: u32, size: usize) -> Vec<u8> {
    let mut out = Vec::new();
    if size == 0 {
        return out;
    }
    let mut current = start;
    let mut guard = 0usize;
    while current < FIRST_RESERVED {
        let Some(blk) = sector(data, sector_size, current) else {
            break;
        };
        out.extend_from_slice(blk);
        if out.len() >= size {
            break;
        }
        current = *fat.get(current as usize).unwrap_or(&END_OF_CHAIN);
        guard += 1;
        if guard > MAX_CHAIN || guard > fat.len() + 1 {
            break;
        }
    }
    out.truncate(size);
    out
}

/// Split a directory chain into entries. `v3` masks stream sizes to 32 bits, as
/// the high half is only meaningful in a version-4 container.
fn parse_directory(bytes: &[u8], v3: bool) -> Vec<Entry> {
    let mut out = Vec::new();
    for chunk in bytes.chunks_exact(DIR_ENTRY_LEN) {
        let kind = chunk[66];
        if kind == 0 {
            continue;
        }
        let name_len = u16_at(chunk, 64).unwrap_or(0) as usize;
        let raw = name_len.saturating_sub(2).min(64);
        let units: Vec<u16> = chunk[..raw]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let name = String::from_utf16_lossy(&units);
        let size = if v3 {
            u32_at(chunk, 120).unwrap_or(0) as u64
        } else {
            u64::from_le_bytes(chunk[120..128].try_into().unwrap_or([0; 8]))
        };
        out.push(Entry {
            name,
            kind,
            start: u32_at(chunk, 116).unwrap_or(END_OF_CHAIN),
            size,
        });
    }
    out
}
