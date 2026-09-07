use std::cmp::min;
use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, Write};

#[derive(Clone, Debug)]
pub(crate) struct Fat32Info {
    root_cluster: u32,
    fat_start: u64,
    data_start: u64,
    cluster_size: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct Fat32Entry {
    pub(crate) name: String,
    pub(crate) attr: u8,
    pub(crate) first_cluster: u32,
    pub(crate) size: u32,
}

impl Fat32Entry {
    pub(crate) fn is_dir(&self) -> bool {
        self.attr & 0x10 != 0
    }
}

pub(crate) fn read_fat32_info(raw: &mut File) -> Result<Fat32Info, String> {
    let mut sector = vec![0u8; 512];
    raw.seek(std::io::SeekFrom::Start(0))
        .map_err(|e| e.to_string())?;
    raw.read_exact(&mut sector).map_err(|e| e.to_string())?;

    let partition_offset = if sector[510] == 0x55 && sector[511] == 0xaa {
        let mut fat32_start = None;
        for index in 0..4 {
            let entry = 446 + index * 16;
            let part_type = sector[entry + 4];
            if part_type == 0x0b || part_type == 0x0c {
                fat32_start = Some(read_u32_le(&sector, entry + 8)? as u64 * 512);
                break;
            }
        }
        fat32_start.unwrap_or(0)
    } else {
        0
    };

    raw.seek(std::io::SeekFrom::Start(partition_offset))
        .map_err(|e| e.to_string())?;
    raw.read_exact(&mut sector).map_err(|e| e.to_string())?;
    if sector[510] != 0x55 || sector[511] != 0xaa {
        return Err("FAT32 boot sector signature is missing".to_string());
    }

    let bytes_per_sector = read_u16_le(&sector, 11)? as u64;
    let sectors_per_cluster = sector[13] as u64;
    let reserved_sectors = read_u16_le(&sector, 14)? as u64;
    let fat_count = sector[16] as u64;
    let root_entries = read_u16_le(&sector, 17)?;
    let fat16_size = read_u16_le(&sector, 22)?;
    let sectors_per_fat = read_u32_le(&sector, 36)? as u64;
    let root_cluster = read_u32_le(&sector, 44)?;
    let fs_type = std::str::from_utf8(&sector[82..90]).unwrap_or("").trim();

    if bytes_per_sector == 0 || sectors_per_cluster == 0 || reserved_sectors == 0 {
        return Err("invalid FAT32 BPB geometry".to_string());
    }
    if root_entries != 0 || fat16_size != 0 || sectors_per_fat == 0 || fs_type != "FAT32" {
        return Err("filesystem is not FAT32".to_string());
    }

    let fat_start = partition_offset + reserved_sectors * bytes_per_sector;
    let data_start = fat_start + fat_count * sectors_per_fat * bytes_per_sector;
    Ok(Fat32Info {
        root_cluster,
        fat_start,
        data_start,
        cluster_size: bytes_per_sector * sectors_per_cluster,
    })
}

pub(crate) fn list_fat32_path(
    raw: &mut File,
    fat: &Fat32Info,
    path: &str,
) -> Result<Vec<Fat32Entry>, String> {
    let cluster = if path.trim_matches(|c| c == '/' || c == '\\').is_empty() {
        fat.root_cluster
    } else {
        let entry = resolve_fat32_path(raw, fat, path)?
            .ok_or_else(|| format!("FAT32 path '{}' not found", path))?;
        if !entry.is_dir() {
            return Err(format!("FAT32 path '{}' is not a directory", path));
        }
        entry.first_cluster
    };

    read_fat32_directory(raw, fat, cluster)
}

pub(crate) fn resolve_fat32_path(
    raw: &mut File,
    fat: &Fat32Info,
    path: &str,
) -> Result<Option<Fat32Entry>, String> {
    let components: Vec<&str> = path
        .trim_matches(|c| c == '/' || c == '\\')
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect();
    if components.is_empty() {
        return Ok(Some(Fat32Entry {
            name: String::from("/"),
            attr: 0x10,
            first_cluster: fat.root_cluster,
            size: 0,
        }));
    }

    let mut cluster = fat.root_cluster;
    for (index, component) in components.iter().enumerate() {
        let entries = read_fat32_directory(raw, fat, cluster)?;
        let Some(entry) = entries
            .into_iter()
            .find(|entry| fat_name_eq(&entry.name, component))
        else {
            return Ok(None);
        };

        if index == components.len() - 1 {
            return Ok(Some(entry));
        }
        if !entry.is_dir() {
            return Ok(None);
        }
        cluster = entry.first_cluster;
    }

    Ok(None)
}

fn read_fat32_directory(
    raw: &mut File,
    fat: &Fat32Info,
    start_cluster: u32,
) -> Result<Vec<Fat32Entry>, String> {
    let clusters = read_fat32_cluster_chain(raw, fat, start_cluster)?;
    let mut entries = Vec::new();
    let mut lfn_parts = Vec::<(u8, String)>::new();
    for cluster in clusters {
        let data = read_fat32_cluster(raw, fat, cluster)?;
        for entry in data.chunks_exact(32) {
            if entry[0] == 0x00 {
                return Ok(entries);
            }
            if entry[0] == 0xe5 {
                lfn_parts.clear();
                continue;
            }

            let attr = entry[11];
            if attr == 0x0f {
                lfn_parts.push((entry[0] & 0x1f, decode_lfn_part(entry)));
                continue;
            }
            if attr & 0x08 != 0 {
                lfn_parts.clear();
                continue;
            }

            let name = if lfn_parts.is_empty() {
                decode_short_name(entry)
            } else {
                lfn_parts.sort_by_key(|(order, _)| *order);
                let name = lfn_parts
                    .iter()
                    .map(|(_, part)| part.as_str())
                    .collect::<String>();
                lfn_parts.clear();
                name
            };

            if name == "." || name == ".." {
                continue;
            }

            let high = read_u16_le(entry, 20)? as u32;
            let low = read_u16_le(entry, 26)? as u32;
            entries.push(Fat32Entry {
                name,
                attr,
                first_cluster: (high << 16) | low,
                size: read_u32_le(entry, 28)?,
            });
        }
    }

    Ok(entries)
}

fn decode_lfn_part(entry: &[u8]) -> String {
    let mut chars = Vec::new();
    for offset in [1usize, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30] {
        let value = u16::from_le_bytes([entry[offset], entry[offset + 1]]);
        if value == 0x0000 || value == 0xffff {
            break;
        }
        chars.push(value);
    }
    String::from_utf16_lossy(&chars)
}

fn decode_short_name(entry: &[u8]) -> String {
    let base = String::from_utf8_lossy(&entry[0..8]).trim().to_string();
    let ext = String::from_utf8_lossy(&entry[8..11]).trim().to_string();
    if ext.is_empty() {
        base
    } else {
        format!("{}.{}", base, ext)
    }
}

fn fat_name_eq(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

pub(crate) fn file_data_extents(
    raw: &mut File,
    fat: &Fat32Info,
    entry: &Fat32Entry,
) -> Result<Vec<(u64, u64)>, String> {
    if entry.size == 0 {
        return Ok(Vec::new());
    }
    if entry.first_cluster < 2 {
        return Err(format!("file '{}' has invalid first cluster", entry.name));
    }

    let clusters = read_fat32_cluster_chain(raw, fat, entry.first_cluster)?;
    let mut remaining = entry.size as u64;
    let mut extents = Vec::new();
    for cluster in clusters {
        if remaining == 0 {
            break;
        }
        let len = min(fat.cluster_size, remaining);
        extents.push((fat32_cluster_offset(fat, cluster), len));
        remaining -= len;
    }
    if remaining != 0 {
        return Err(format!(
            "file '{}' FAT chain is shorter than size",
            entry.name
        ));
    }
    Ok(extents)
}

pub(crate) fn write_replacement_to_raw(
    raw: &mut File,
    extents: &[(u64, u64)],
    replacement: &[u8],
) -> Result<(), String> {
    let mut source_offset = 0usize;
    for (offset, len) in extents {
        let len = *len as usize;
        raw.seek(std::io::SeekFrom::Start(*offset))
            .map_err(|e| e.to_string())?;
        raw.write_all(&replacement[source_offset..source_offset + len])
            .map_err(|e| e.to_string())?;
        source_offset += len;
    }
    Ok(())
}

pub(crate) fn read_file_extents(raw: &mut File, extents: &[(u64, u64)]) -> Result<Vec<u8>, String> {
    let total_len = extents.iter().map(|(_, len)| *len as usize).sum();
    let mut data = Vec::with_capacity(total_len);
    for (offset, len) in extents {
        let start = data.len();
        data.resize(start + *len as usize, 0);
        raw.seek(std::io::SeekFrom::Start(*offset))
            .map_err(|e| e.to_string())?;
        raw.read_exact(&mut data[start..])
            .map_err(|e| e.to_string())?;
    }
    Ok(data)
}

fn read_fat32_cluster_chain(
    raw: &mut File,
    fat: &Fat32Info,
    start_cluster: u32,
) -> Result<Vec<u32>, String> {
    if start_cluster < 2 {
        return Ok(Vec::new());
    }

    let mut clusters = Vec::new();
    let mut seen = HashSet::new();
    let mut cluster = start_cluster;
    loop {
        if !seen.insert(cluster) {
            return Err(format!("FAT32 cluster chain loop at {}", cluster));
        }
        clusters.push(cluster);

        let entry_offset = fat.fat_start + cluster as u64 * 4;
        let mut entry = [0u8; 4];
        raw.seek(std::io::SeekFrom::Start(entry_offset))
            .map_err(|e| e.to_string())?;
        raw.read_exact(&mut entry).map_err(|e| e.to_string())?;
        let next = u32::from_le_bytes(entry) & 0x0fff_ffff;
        if next >= 0x0fff_fff8 {
            break;
        }
        if next == 0 || next == 1 || next == 0x0fff_fff7 {
            return Err(format!("invalid FAT32 next cluster {}", next));
        }
        cluster = next;
    }

    Ok(clusters)
}

fn read_fat32_cluster(raw: &mut File, fat: &Fat32Info, cluster: u32) -> Result<Vec<u8>, String> {
    let mut data = vec![0u8; fat.cluster_size as usize];
    raw.seek(std::io::SeekFrom::Start(fat32_cluster_offset(fat, cluster)))
        .map_err(|e| e.to_string())?;
    raw.read_exact(&mut data).map_err(|e| e.to_string())?;
    Ok(data)
}

fn fat32_cluster_offset(fat: &Fat32Info, cluster: u32) -> u64 {
    fat.data_start + (cluster as u64 - 2) * fat.cluster_size
}

fn read_u16_le(data: &[u8], offset: usize) -> Result<u16, String> {
    if offset + 2 > data.len() {
        return Err("unexpected end of file while reading u16".to_string());
    }
    Ok(u16::from_le_bytes([data[offset], data[offset + 1]]))
}

fn read_u32_le(data: &[u8], offset: usize) -> Result<u32, String> {
    if offset + 4 > data.len() {
        return Err("unexpected end of file while reading u32".to_string());
    }
    Ok(u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ]))
}
