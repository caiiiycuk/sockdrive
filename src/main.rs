use std::cmp::min;
use std::collections::{HashMap, HashSet};
use std::fs::{create_dir, create_dir_all, metadata, read_dir, remove_file, rename, write, File};
use std::io::{Read, Seek, Write};
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

mod doctor;
mod fat32;

const AHEAD_READ_SIZE: u64 = 256 * 1024;
const FAT16_256MB: &str = include_str!("../drives/fat16-256mb.json");
const FAT32_2GB: &str = include_str!("../drives/fat32-2gb.json");
const SMALL_FILES_THRESHOLD: u64 = 1024 * 1024;
const DEFAULT_PRELOAD: &str = "0,16,1,52,50,68,51,145,280,152,291,227,234,226,207,279,7195,257,233,179,231,390,177,346,66,71,96,197,297,70,90,113,146,87,89,98,2,93,199,236,54,198,129,228,296,299,311,180,20,100,208,218,219,232,276,300,24,114,143,195,229,239,253,241,277,289,49,155,240,21,23,99,116,151,217,97,202,429,32,157,262,327,200,201,25,156,237,278,329,82,141,142,154,158,178,338,339,84,78,65,148,160,271,282,117,119,144,275,83,85,92,3,159,242,274,105,118,543,64,187,261,269,86,225,545,22,38,57,188,287,330,176,359,544,56,281,295,245,79,30,407,165,194,235,285,465,101,238,411,58,138,193,293,394,133,134,168,412,6,55,62,163,333,343,112,172,428,430,17,18,19,67,184,332,171,104,7,36,284,334,386,395,139,167,357,431,37,76,140,460,244,258,331,532,290,12,13,14,69,153,272,328,396,461,675,175,663,664,149,405,531,15,123,63,464,53,31,221,252,294,340,344,35,60,72,288,246,459,462,463,4,513,546,677,9,75,94,164,216,251,363,220,658,701,397,59,196,230,354,364,667,323,533,729";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2
        || (args[1] != "mkd" && args[1] != "sockify" && args[1] != "doctor" && args[1] != "docktor")
    {
        eprintln!(
            "
sockdrive cl

Use one of the following commands:
    sockdrive mkd   - make sockdrive from raw / qcow2 image
    sockdrive sockify - transform jsdos bundle with qcow2 images to use sockdrive
    sockdrive doctor - download, inspect and patch sockdrive mounts
            "
        );
    } else if args[1] == "mkd" {
        mkd(args);
    } else if args[1] == "sockify" {
        sockify(args);
    } else if args[1] == "doctor" || args[1] == "docktor" {
        doctor::doctor(args);
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SockdriveMount {
    pub(crate) drive: String,
    pub(crate) url: String,
}

#[derive(Clone, Debug)]
pub(crate) struct SockdriveMeta {
    pub(crate) size_kb: u64,
    pub(crate) ahead_read: u64,
    pub(crate) range_count: u64,
    pub(crate) sector_size: u64,
    pub(crate) dropped_ranges: Vec<u32>,
    pub(crate) small_ranges: Vec<u32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StoredEncoding {
    Plain,
    Gzip,
    Brotli,
}

pub(crate) fn find_sockdrive_mounts(dosbox_conf: &str) -> Vec<SockdriveMount> {
    dosbox_conf
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if !trimmed.starts_with("imgmount") {
                return None;
            }

            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() < 4 || parts[2] != "sockdrive" {
                return None;
            }

            Some(SockdriveMount {
                drive: parts[1].to_string(),
                url: normalize_sockdrive_url(parts[3]),
            })
        })
        .collect()
}

pub(crate) fn normalize_sockdrive_url(url: &str) -> String {
    let mut normalized = url
        .replace(
            "wss://sockdrive.js-dos.com:8001/dos.zone/",
            "https://br.cdn.dos.zone/sockdrive-qcow2/dos.zone-",
        )
        .replace(
            "wss://sockdrive.js-dos.com:8001/system/",
            "https://br.cdn.dos.zone/sockdrive-qcow2/system-",
        );

    while normalized.ends_with('/') {
        normalized.pop();
    }

    normalized
}

pub(crate) fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

pub(crate) fn parse_sockdrive_meta(meta: &serde_json::Value) -> Result<SockdriveMeta, String> {
    Ok(SockdriveMeta {
        size_kb: required_u64(meta, "size")?,
        ahead_read: required_u64(meta, "ahead_read")?,
        range_count: required_u64(meta, "range_count")?,
        sector_size: required_u64(meta, "sector_size")?,
        dropped_ranges: optional_u32_array(meta, "dropped_ranges")?,
        small_ranges: optional_u32_array(meta, "small_ranges")?,
    })
}

fn required_u64(meta: &serde_json::Value, field: &str) -> Result<u64, String> {
    meta.get(field)
        .and_then(|value| value.as_u64())
        .ok_or_else(|| format!("sockdrive.metaj field '{}' is missing or invalid", field))
}

pub(crate) fn optional_u32_array(
    meta: &serde_json::Value,
    field: &str,
) -> Result<Vec<u32>, String> {
    let Some(value) = meta.get(field) else {
        return Ok(Vec::new());
    };
    let array = value
        .as_array()
        .ok_or_else(|| format!("sockdrive.metaj field '{}' should be an array", field))?;
    array
        .iter()
        .map(|value| {
            value
                .as_u64()
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| format!("sockdrive.metaj field '{}' contains invalid range", field))
        })
        .collect()
}

pub(crate) fn validate_sockdrive_meta(meta: &SockdriveMeta) -> Result<(), String> {
    if meta.size_kb == 0 {
        return Err("sockdrive.metaj size should be greater than zero".to_string());
    }
    if meta.ahead_read == 0 {
        return Err("sockdrive.metaj ahead_read should be greater than zero".to_string());
    }
    if meta.sector_size == 0 {
        return Err("sockdrive.metaj sector_size should be greater than zero".to_string());
    }

    let expected_range_count = (meta.size_kb * 1024).div_ceil(meta.ahead_read);
    if expected_range_count != meta.range_count {
        return Err(format!(
            "range_count mismatch: {} != computed {}",
            meta.range_count, expected_range_count
        ));
    }

    let mut seen = HashSet::new();
    for range in meta.dropped_ranges.iter().chain(meta.small_ranges.iter()) {
        if *range as u64 >= meta.range_count {
            return Err(format!(
                "range {} is outside range_count {}",
                range, meta.range_count
            ));
        }
        if !seen.insert(*range) {
            return Err(format!("range {} is duplicated in metaj lists", range));
        }
    }

    Ok(())
}

pub(crate) fn download_sockdrive_file(url: &str, name: &str, output: &Path) -> Result<(), String> {
    let file_url = format!("{}/{}", url, name);
    let status = Command::new("curl")
        .args([
            "-f",
            "-s",
            "-S",
            "-L",
            "--compressed",
            "-H",
            "Accept-Encoding: br,gzip",
            "-o",
            &output.to_string_lossy(),
            &file_url,
        ])
        .status()
        .map_err(|e| format!("failed to run curl: {}", e))?;

    if !status.success() {
        return Err(format!("failed to download {}", file_url));
    }

    Ok(())
}

pub(crate) fn decode_json_file(path: &Path) -> Result<(Vec<u8>, StoredEncoding), String> {
    for (candidate, encoding) in decode_candidates(path)? {
        if serde_json::from_slice::<serde_json::Value>(&candidate).is_ok() {
            return Ok((candidate, encoding));
        }
    }

    Err(format!("file '{}' is not valid JSON", path.display()))
}

pub(crate) fn decode_file_len(
    path: &Path,
    expected_len: usize,
) -> Result<(Vec<u8>, StoredEncoding), String> {
    let candidates = decode_candidates(path)?;
    for (candidate, encoding) in candidates
        .iter()
        .filter(|(_, encoding)| *encoding != StoredEncoding::Plain)
    {
        if candidate.len() == expected_len {
            return Ok((candidate.clone(), *encoding));
        }
    }

    for (candidate, encoding) in candidates {
        if candidate.len() == expected_len {
            return Ok((candidate, encoding));
        }
    }

    Err(format!(
        "file '{}' does not decode to expected size {}",
        path.display(),
        expected_len
    ))
}

fn decode_candidates(path: &Path) -> Result<Vec<(Vec<u8>, StoredEncoding)>, String> {
    let original = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut candidates = vec![(original, StoredEncoding::Plain)];

    if let Some(decoded) = decode_with_command("gzip", &["-d", "-c"], path)? {
        candidates.push((decoded, StoredEncoding::Gzip));
    }

    if let Some(decoded) = decode_with_command("brotli", &["-d", "-c"], path)? {
        candidates.push((decoded, StoredEncoding::Brotli));
    }

    Ok(candidates)
}

fn decode_with_command(cmd: &str, args: &[&str], path: &Path) -> Result<Option<Vec<u8>>, String> {
    let mut command = Command::new(cmd);
    command.args(args).arg(path);
    let output = command
        .output()
        .map_err(|e| format!("failed to run {}: {}", cmd, e))?;

    if output.status.success() {
        Ok(Some(output.stdout))
    } else {
        Ok(None)
    }
}

pub(crate) fn encode_file(
    path: &Path,
    data: &[u8],
    encoding: StoredEncoding,
) -> Result<(), String> {
    match encoding {
        StoredEncoding::Plain => std::fs::write(path, data).map_err(|e| e.to_string()),
        StoredEncoding::Gzip => encode_file_with_command(path, data, "gzip", "-9", "gz"),
        StoredEncoding::Brotli => encode_file_with_command(path, data, "brotli", "-Z", "br"),
    }
}

fn encode_file_with_command(
    path: &Path,
    data: &[u8],
    command_name: &str,
    compression_arg: &str,
    suffix: &str,
) -> Result<(), String> {
    let tmp = path.with_extension(format!(
        "encode-tmp-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let compressed_tmp = tmp.with_extension(format!(
        "{}.{}",
        tmp.extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("encode-tmp"),
        suffix
    ));

    std::fs::write(&tmp, data).map_err(|e| e.to_string())?;
    let status = Command::new(command_name)
        .arg(compression_arg)
        .arg("-k")
        .arg(&tmp)
        .status()
        .map_err(|e| format!("failed to run {}: {}", command_name, e))?;
    if !status.success() {
        let _ = remove_file(&tmp);
        return Err(format!("{} failed", command_name));
    }

    if path.exists() {
        remove_file(path).map_err(|e| e.to_string())?;
    }
    rename(&compressed_tmp, path).map_err(|e| e.to_string())?;
    remove_file(&tmp).map_err(|e| e.to_string())?;
    Ok(())
}

pub(crate) fn parse_sockdrive_changes(encoded: &[u8]) -> Result<HashMap<String, Vec<u8>>, String> {
    let mut changes = HashMap::new();
    let mut offset = 0usize;
    while offset < encoded.len() {
        let url_len = read_u32_le(encoded, offset)? as usize;
        offset += 4;
        if url_len > 4096 {
            return Err("url length is greater than 4096".to_string());
        }
        if offset + url_len > encoded.len() {
            return Err("url length exceeds file size".to_string());
        }
        let url = std::str::from_utf8(&encoded[offset..offset + url_len])
            .map_err(|e| format!("invalid url utf8: {}", e))?
            .to_string();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(format!("invalid changes url '{}'", url));
        }
        offset += url_len;

        let persist_len = read_u32_le(encoded, offset)? as usize;
        offset += 4;
        if offset + persist_len > encoded.len() {
            return Err("persist length exceeds file size".to_string());
        }

        changes.insert(
            normalize_sockdrive_url(&url),
            encoded[offset..offset + persist_len].to_vec(),
        );
        offset += persist_len;
    }

    Ok(changes)
}

pub(crate) fn apply_sockdrive_changes(
    raw_path: &Path,
    changes: &[u8],
    meta: &SockdriveMeta,
) -> Result<(), String> {
    let mut raw = File::options()
        .read(true)
        .write(true)
        .open(raw_path)
        .map_err(|e| e.to_string())?;
    let sectors = deserialize_sectors(changes, meta)?;
    for (sector, data) in sectors {
        let offset = sector * meta.sector_size;
        if offset + meta.sector_size > meta.size_kb * 1024 {
            return Err(format!("changed sector {} is outside raw image", sector));
        }
        raw.seek(std::io::SeekFrom::Start(offset))
            .map_err(|e| e.to_string())?;
        raw.write_all(&data).map_err(|e| e.to_string())?;
    }

    Ok(())
}

fn deserialize_sectors(data: &[u8], meta: &SockdriveMeta) -> Result<Vec<(u64, Vec<u8>)>, String> {
    let count = read_u32_le(data, 0)? as usize;
    let chunk_size = meta.sector_size as usize + 4;
    let mut offset = 4usize;
    let mut sectors = Vec::with_capacity(count);

    for chunk_index in 0..count {
        let compressed_size = read_u32_le(data, offset)? as usize;
        offset += 4;
        if offset + compressed_size > data.len() {
            return Err(format!(
                "chunk {} exceeds changes payload size",
                chunk_index
            ));
        }
        let compressed_chunk = &data[offset..offset + compressed_size];
        offset += compressed_size;

        let chunk = if compressed_size == chunk_size {
            compressed_chunk.to_vec()
        } else {
            lz4_uncompress(compressed_chunk, chunk_size)?
        };
        if chunk.len() != chunk_size {
            return Err(format!(
                "chunk {} size mismatch: {} != {}",
                chunk_index,
                chunk.len(),
                chunk_size
            ));
        }

        let sector = read_u32_le(&chunk, 0)? as u64;
        sectors.push((sector, chunk[4..].to_vec()));
    }

    if offset != data.len() {
        return Err("changes payload has trailing bytes".to_string());
    }

    Ok(sectors)
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

fn lz4_uncompress(input: &[u8], output_len: usize) -> Result<Vec<u8>, String> {
    let mut output = vec![0u8; output_len];
    let mut i = 0usize;
    let mut j = 0usize;

    while i < input.len() {
        let token = input[i];
        i += 1;

        let mut literals_length = (token >> 4) as usize;
        if literals_length > 0 {
            let mut len = literals_length + 240;
            while len == 255 {
                if i >= input.len() {
                    return Err("invalid lz4 literal length".to_string());
                }
                len = input[i] as usize;
                i += 1;
                literals_length += len;
            }

            if i + literals_length > input.len() || j + literals_length > output.len() {
                return Err("invalid lz4 literal copy".to_string());
            }
            output[j..j + literals_length].copy_from_slice(&input[i..i + literals_length]);
            i += literals_length;
            j += literals_length;

            if i == input.len() {
                return Ok(output[..j].to_vec());
            }
        }

        if i + 2 > input.len() {
            return Err("invalid lz4 offset".to_string());
        }
        let match_offset = input[i] as usize | ((input[i + 1] as usize) << 8);
        i += 2;
        if match_offset == 0 || match_offset > j {
            return Err("invalid lz4 match offset".to_string());
        }

        let mut match_length = (token & 0x0f) as usize;
        let mut len = match_length + 240;
        while len == 255 {
            if i >= input.len() {
                return Err("invalid lz4 match length".to_string());
            }
            len = input[i] as usize;
            i += 1;
            match_length += len;
        }
        match_length += 4;

        if j + match_length > output.len() {
            return Err("invalid lz4 match copy".to_string());
        }
        for _ in 0..match_length {
            output[j] = output[j - match_offset];
            j += 1;
        }
    }

    Ok(output[..j].to_vec())
}

pub(crate) fn convert_raw_to_qcow2(raw_path: &Path, qcow2_path: &Path) -> Result<(), String> {
    let status = Command::new("qemu-img")
        .args([
            "convert",
            "-f",
            "raw",
            "-O",
            "qcow2",
            &raw_path.to_string_lossy(),
            &qcow2_path.to_string_lossy(),
        ])
        .status()
        .map_err(|e| format!("failed to run qemu-img: {}", e))?;

    if !status.success() {
        return Err("qemu-img convert failed".to_string());
    }

    Ok(())
}

fn sockify(args: Vec<String>) {
    if args.len() < 6 || args[1] != "sockify" {
        eprintln!(
            "
sockdrive cli: sockify

Usage:
    sockdrive sockify <jsdos_bundle> <output_dir> <url> <sockified_bundle>

    jsdos_bundle: path to the jsdos bundle file (or directory with bundle files)
    drive_prefix: prefix for generated drives (e.g. 'gamename-')
    output_dir: path to the output directory (where sockdrive files will be placed)
    url: url to the sockdrive server (where you need to put sockdrive files)
    sockified_bundle: path to the resulting jsdos bundle file
    -b: enable brotli compression (brotli cmd should be in PATH)
    -g: enable gzip compression (gzip cmd should be in PATH)

Example:
    sockdrive sockify bundle.jsdos bundle- ./sockdrive https://my.site bundle-sockified.jsdos [-b] [-g]
    OR
    sockdrive sockify bindle-dir bundle- ./sockdrive https://my.site bindle-sockified.jsdos [-b] [-g]

Note:
    in our example you need to publish context of ./s3 folder to your web-server,
    they should be available at https://my.site/sockdrive
        "
        );

        std::process::exit(1);
    }

    let jsdos_bundle = &args[2];
    let drive_prefix = &args[3];
    let output_dir = &args[4];
    let url = &args[5];
    let sockified_bundle = &args[6];
    let temp_dir = format!("{}/sockdrive-temp", output_dir);

    let url = if url.ends_with("/") {
        url[..url.len() - 1].to_string()
    } else {
        url.to_string()
    };

    let output_dir = if let Some(stripped) = output_dir.strip_prefix("./") {
        stripped.to_string()
    } else {
        output_dir.to_string()
    };

    if std::path::Path::new(&temp_dir).exists() {
        std::fs::remove_dir_all(&temp_dir).unwrap();
    }

    if std::path::Path::new(&sockified_bundle).exists() {
        eprintln!("Error: sockified_bundle '{}' exists", sockified_bundle);
        std::process::exit(1);
    }

    create_dir_all(&temp_dir).unwrap();
    let cleanup = || {
        std::fs::remove_dir_all(&temp_dir).unwrap();
    };

    if std::path::Path::new(jsdos_bundle).is_dir() {
        copy_dir_all(jsdos_bundle, &temp_dir).unwrap();
    } else {
        Command::new("7z")
            .args(["x", jsdos_bundle, &format!("-o{}", temp_dir)])
            .status()
            .unwrap();
    }

    let dosbox_conf = format!("{}/.jsdos/dosbox.conf", temp_dir);
    if !std::path::Path::new(&dosbox_conf).exists() {
        eprintln!("Error: dosbox.conf '{}' does not exist", dosbox_conf);
        cleanup();
        std::process::exit(1);
    }

    let dosbox_conf_content = std::fs::read_to_string(&dosbox_conf).unwrap();
    let mut dosbox_conf_content: Vec<String> =
        dosbox_conf_content.lines().map(|s| s.to_string()).collect();
    let imgmount_lines: Vec<(usize, String)> = dosbox_conf_content
        .iter()
        .enumerate()
        .filter(|(_, line)| line.trim().starts_with("imgmount") && line.contains(".qcow2"))
        .map(|(i, line)| (i, line.clone()))
        .collect();

    if imgmount_lines.is_empty() {
        eprintln!("Error: qcow2 mounts not found in dosbox.conf");
        cleanup();
        std::process::exit(1);
    }

    for (i, line) in imgmount_lines {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            eprintln!("Error: imgmount line '{}' is invalid", line);
            cleanup();
            std::process::exit(1);
        }

        let drive = parts[1];
        let path = parts[2];
        let indrive = format!("{}/{}", temp_dir, path);
        let outname = std::path::Path::new(path)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let outname = format!("{}{}", drive_prefix, outname);
        let outdrive = format!("{}/{}", output_dir, outname);

        println!("outdrive: {}, outname: {}", outdrive, outname);

        if std::path::Path::new(&outdrive).exists() {
            eprintln!("Error: drive '{}' already exists", outdrive);
            cleanup();
            std::process::exit(1);
        }

        if args.contains(&"-b".to_string()) || args.contains(&"-g".to_string()) {
            mkd(vec![
                "_".to_string(),
                "mkd".to_string(),
                indrive.clone(),
                "_".to_string(),
                outdrive.clone(),
                if args.contains(&"-b".to_string()) {
                    "-b".to_string()
                } else {
                    "-g".to_string()
                },
            ]);
        } else {
            mkd(vec![
                "_".to_string(),
                "mkd".to_string(),
                indrive.clone(),
                "_".to_string(),
                outdrive.clone(),
            ]);
        }

        dosbox_conf_content[i] = format!("imgmount {} sockdrive {}/{}", drive, url, outname);
        std::fs::remove_file(&indrive).unwrap();
    }

    std::fs::write(&dosbox_conf, dosbox_conf_content.join("\n")).unwrap();

    Command::new("7z")
        .args([
            "a",
            "-tzip",
            "-mx0",
            sockified_bundle,
            &format!("{}/.", temp_dir),
        ])
        .status()
        .unwrap();

    cleanup();
}

fn mkd(args: Vec<String>) {
    if args.len() < 5 || args[1] != "mkd" {
        eprintln!(
            "
sockdrive cli: makedrive

Usage:
    sockdrive mkd <raw_image|qcow2_image> <preload_ranges> <output_dir> [-b]

    raw_image|qcow2_image: path to the raw image file or qcow2 image file
    preload_ranges: comma separated list of ranges to preload on startup (range is index, range size is AHEAD_READ_SIZE(256 * 1024))
    output_dir: path to the output directory
    -b: enable brotli compression (brotli cmd should be in PATH)
    -g: enable gzip compression (gzip cmd should be in PATH)

Note 1:
    use '_' as a default preload ranges

Note 2:
    if you use qcow2 image then you must change permissions of /boot/vmlinuz-*
    sudo chmod +r /boot/vmlinuz-*
    
    more:
    https://askubuntu.com/questions/1046828/how-to-run-libguestfs-tools-tools-such-as-virt-make-fs-without-sudo

Example:
    sockdrive mkd win95v1.raw _ ./output [-b] [-g]
        "
        );
        std::process::exit(1);
    }

    let input_file = &args[2];
    let preload = &args[3];
    let output_dir = &args[4];

    if !std::path::Path::new(&input_file).exists() {
        eprintln!("Error: Input file '{}' does not exist", input_file);
        std::process::exit(1);
    }

    let input_file = if input_file.ends_with(".qcow2") || input_file.ends_with(".qcow") {
        println!("Scanning the qcow2 image for errors");
        let boot_img = format!(
            "{}/boot.img",
            std::env::current_exe().unwrap().parent().unwrap().display()
        );
        if !std::path::Path::new(&boot_img).exists() {
            eprintln!("Error: boot.img '{}' does not exist", boot_img);
            std::process::exit(1);
        }
        Command::new("qemu-system-i386")
            .args([
                "-boot",
                "a",
                "-fda",
                &boot_img,
                "-hda",
                input_file,
                "-display",
                "none",
                "--no-reboot",
            ])
            .status()
            .expect("Failed to run qemu scandisk");

        println!("Converting qcow2 image to raw image");
        let raw = format!("{}.raw", input_file);
        Command::new("virt-sparsify")
            .args(["--convert", "raw", input_file, &raw])
            .status()
            .expect("Failed to convert qcow2 to raw");
        raw
    } else {
        input_file.to_owned()
    };

    let input_size = metadata(&input_file).unwrap().len();
    let fat16_256mb: serde_json::Value = serde_json::from_str(FAT16_256MB).unwrap();
    let fat32_2gb: serde_json::Value = serde_json::from_str(FAT32_2GB).unwrap();
    let fat16_256mb_size = fat16_256mb.get("size").unwrap().as_u64().unwrap() * 1024;
    let fat32_2gb_size = fat32_2gb.get("size").unwrap().as_u64().unwrap() * 1024;

    let mut config = if input_size == fat16_256mb_size {
        fat16_256mb
    } else if input_size == fat32_2gb_size {
        fat32_2gb
    } else {
        eprintln!("Error: Input file size ({}) should match to one of templates (FAT16-256MB: {} or FAT32-2GB: {})", 
            input_size,
            fat16_256mb_size, fat32_2gb_size);
        std::process::exit(1);
    };

    let mut range_count = input_size / AHEAD_READ_SIZE;
    if range_count * AHEAD_READ_SIZE < input_size {
        range_count += 1;
    }

    config
        .as_object_mut()
        .unwrap()
        .insert(String::from("range_count"), serde_json::json!(range_count));

    config.as_object_mut().unwrap().insert(
        String::from("ahead_read"),
        serde_json::json!(AHEAD_READ_SIZE),
    );

    if std::path::Path::new(&output_dir).exists() {
        eprintln!("Error: Output directory '{}' exists", output_dir);
        std::process::exit(1);
    };

    create_dir(output_dir).unwrap();

    let dropped = mkahead(&input_file, range_count as u32, output_dir);
    config
        .as_object_mut()
        .unwrap()
        .insert(String::from("dropped_ranges"), serde_json::json!(dropped));

    let preload_ranges: Vec<u32> = if preload == "_" {
        DEFAULT_PRELOAD
    } else {
        preload
    }
    .split(',')
    .filter_map(|s| s.trim().parse().ok())
    .filter(|range| *range < range_count as u32)
    .filter(|range| !dropped.contains(range))
    .collect();

    config.as_object_mut().unwrap().insert(
        String::from("preload_ranges"),
        serde_json::json!(preload_ranges),
    );

    write(
        format!("{}/sockdrive.metaj", output_dir),
        serde_json::to_string(&config).unwrap(),
    )
    .unwrap();

    println!(
        "Done, created {} files in {}",
        range_count - dropped.len() as u64,
        output_dir
    );

    if args.contains(&"-b".to_string()) {
        compress_all(output_dir, "brotli", "-Z", "-0", "br");
        reduce_small_files(output_dir, &mut config, "brotli", "-Z", "br");
    } else if args.contains(&"-g".to_string()) {
        compress_all(output_dir, "gzip", "-9", "-1", "gz");
        reduce_small_files(output_dir, &mut config, "gzip", "-9", "gz");
    }

    if args[2] != input_file {
        remove_file(&input_file).unwrap();
    }
}

fn mkahead(raw_image: &str, range_count: u32, output_dir: &str) -> Vec<u32> {
    let mut raw = File::open(raw_image).unwrap();
    let mut buffer = vec![0u8; AHEAD_READ_SIZE as usize];
    let mut dropped = Vec::new();
    let raw_size = raw.metadata().unwrap().len() as usize;

    raw.seek(std::io::SeekFrom::Start(0)).unwrap();
    for i in 0..range_count {
        if ((i + 1) as u64 * AHEAD_READ_SIZE) as usize > raw_size {
            buffer.fill(0);
            raw.read_exact(&mut buffer[..raw_size - i as usize * AHEAD_READ_SIZE as usize])
                .unwrap();
        } else {
            raw.read_exact(&mut buffer).unwrap();
        }

        if buffer.iter().any(|x| *x != 0) {
            let mut ahead_file = File::create(format!("{}/{}.raw", output_dir, i)).unwrap();
            ahead_file.write_all(&buffer).unwrap();
        } else {
            dropped.push(i);
        }
    }

    dropped
}

fn compress_all(
    output_dir: &str,
    compression_cmd: &str,
    compression_best: &str,
    compression_fast: &str,
    compression_suffix: &str,
) {
    let files: Vec<_> = read_dir(output_dir).unwrap().flatten().collect();
    let num_cpus = num_cpus::get();
    let chunks = files.chunks(files.len().div_ceil(num_cpus));

    let total = chunks.len();
    let compression_cmd = compression_cmd.to_string();
    let compression_best = compression_best.to_string();
    let compression_fast = compression_fast.to_string();
    let compression_suffix = compression_suffix.to_string();

    println!("Compressing {} files on {} CPUs", files.len(), num_cpus);
    chunks.enumerate().for_each(|(i, chunk)| {
        let handles: Vec<_> = chunk
            .iter()
            .map(|file| {
                let path = file.path();
                let compression_cmd = compression_cmd.clone();
                let compression_best = compression_best.clone();
                let compression_fast = compression_fast.clone();
                let compression_suffix = compression_suffix.clone();

                std::thread::spawn(move || {
                    let status = Command::new(&compression_cmd)
                        .arg(compression_best)
                        .arg("-k")
                        .arg(&path)
                        .status()
                        .unwrap();

                    if !status.success() {
                        eprintln!("Failed to compress {:?}", path);
                        std::process::exit(1);
                    }

                    let compressed_path = format!("{}.{}", &path.display(), compression_suffix);
                    let orig_size = metadata(&path).unwrap().len();
                    let compressed_size = match metadata(&compressed_path) {
                        Ok(meta) => meta.len(),
                        Err(_) => {
                            println!("Failed to get metadata for {:?}", compressed_path);
                            std::process::exit(1);
                        }
                    };

                    if compressed_size < orig_size {
                        remove_file(&path).unwrap();
                        rename(compressed_path, path).unwrap();
                    } else {
                        remove_file(&compressed_path).unwrap();
                        let status = Command::new(compression_cmd)
                            .arg(compression_fast)
                            .arg("-k")
                            .arg(&path)
                            .status()
                            .unwrap();

                        if !status.success() {
                            eprintln!("Failed to compress {:?}", path);
                            std::process::exit(1);
                        }

                        remove_file(&path).unwrap();
                        rename(compressed_path, path).unwrap();
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        println!("Processed {}%", i * 100 / total);
    });
}

fn reduce_small_files(
    output_dir: &str,
    metaj: &mut serde_json::Value,
    compression_cmd: &str,
    compression_best: &str,
    compression_suffix: &str,
) {
    let files: Vec<_> = read_dir(output_dir).unwrap().flatten().collect();
    let mut all_files = Vec::new();

    for file in files {
        if file.path().to_string_lossy().ends_with("metaj") || file.path().ends_with("preload.raw")
        {
            continue;
        }

        let path = file.path();
        let size = metadata(&path).unwrap().len();
        assert!(
            size <= AHEAD_READ_SIZE + 100,
            "{}, range size should be less than AHEAD_READ_SIZE: {} > {}",
            &path.display(),
            size,
            AHEAD_READ_SIZE
        );
        all_files.push((path, size));
    }

    all_files.sort_by_key(|(_, size)| *size);

    let mut small_files = Vec::new();
    let mut total_size = 0;

    for (path, size) in all_files {
        if total_size + size > SMALL_FILES_THRESHOLD {
            break;
        }

        total_size += min(size, AHEAD_READ_SIZE);
        small_files.push(path);
    }

    if !small_files.is_empty() {
        let mut file_locations = Vec::new();
        let mut file_contents = Vec::<u8>::new();

        for path in &small_files {
            let decoded_path = format!("{}/decoded.raw", output_dir);
            let status = Command::new(compression_cmd)
                .arg("-dkc")
                .arg(path)
                .stdout(File::create(&decoded_path).unwrap())
                .status()
                .unwrap();

            if !status.success() {
                eprintln!("Failed to decompress preload file");
                std::process::exit(1);
            }

            let mut file = File::open(&decoded_path).unwrap();
            let offset = file_contents.len() as u64;
            file.read_to_end(&mut file_contents).unwrap();
            let size = file_contents.len() - offset as usize;
            assert!(
                size == AHEAD_READ_SIZE as usize,
                "file size should be AHEAD_READ_SIZE: {} != {}",
                size,
                AHEAD_READ_SIZE
            );

            file_locations.push(
                path.file_stem()
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
                    .parse::<i32>()
                    .unwrap(),
            );
            remove_file(&decoded_path).unwrap();
            remove_file(path).unwrap();
        }

        let preload_file_str = format!("{}/preload.raw", output_dir);
        let mut preload_file = File::create(&preload_file_str).unwrap();
        preload_file.write_all(&file_contents).unwrap();

        println!("Found {} small files, compressed size: {} kb, computed size: {} kb, preload file size: {} kb", 
            small_files.len(),
            total_size / 1024,
            small_files.len() * AHEAD_READ_SIZE as usize / 1024,
            file_contents.len() / 1024);

        let status = Command::new(compression_cmd)
            .arg(compression_best)
            .arg("-k")
            .arg(&preload_file_str)
            .status()
            .unwrap();

        if !status.success() {
            eprintln!("Failed to compress preload file");
            std::process::exit(1);
        }

        remove_file(&preload_file_str).unwrap();
        rename(
            format!("{}/preload.raw.{}", output_dir, compression_suffix),
            &preload_file_str,
        )
        .unwrap();
        println!(
            "Preload file size: {} kb",
            metadata(&preload_file_str).unwrap().len() / 1024
        );

        metaj.as_object_mut().unwrap().insert(
            String::from("small_ranges"),
            serde_json::json!(file_locations),
        );

        let metaj_file = format!("{}/sockdrive.metaj", output_dir);
        write(&metaj_file, serde_json::to_string(&metaj).unwrap()).unwrap();

        let status = Command::new(compression_cmd)
            .arg(compression_best)
            .arg("-k")
            .arg(&metaj_file)
            .status()
            .unwrap();

        if !status.success() {
            eprintln!("Failed to compress preload file");
            std::process::exit(1);
        }

        remove_file(&metaj_file).unwrap();
        rename(
            format!("{}/sockdrive.metaj.{}", output_dir, compression_suffix),
            metaj_file,
        )
        .unwrap();
    }
}

pub(crate) fn copy_dir_all(
    src: impl AsRef<std::path::Path>,
    dst: impl AsRef<std::path::Path>,
) -> std::io::Result<()> {
    std::fs::create_dir_all(&dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(entry.path(), dst.as_ref().join(entry.file_name()))?;
        } else {
            std::fs::copy(entry.path(), dst.as_ref().join(entry.file_name()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctor::{
        doctor_extract_file_to, doctor_output_dir, doctor_patch_file, read_doctor_mount,
        restore_sockdrive_raw_from_dir, write_doctor_manifest, DoctorMount,
    };
    use crate::fat32::{file_data_extents, list_fat32_path, read_fat32_info, resolve_fat32_path};
    use std::fs::remove_dir_all;
    use std::path::{Path, PathBuf};

    #[test]
    fn test_fat16_256mb_exists() {
        let path = Path::new("test-assets/fat16-256mb.raw");
        assert!(path.exists(), "fat16-256mb.raw file should exist, please run `./test-assets/generate.sh` from root to generate it");
    }

    #[test]
    fn test_fat32_2gb_exists() {
        let path = Path::new("test-assets/fat32-2gb.raw");
        assert!(path.exists(), "fat32-2gb.raw file should exist, please run `./test-assets/generate.sh` from root to generate it");
    }

    #[test]
    fn test_mkd_fat16_256mb() {
        test_mkd(
            "test-assets/fat16-256mb.raw",
            &[0, 1, 2],
            "test-assets/fat16-256mb",
        );
    }

    #[test]
    fn test_mkd_fat32_2gb() {
        test_mkd(
            "test-assets/fat32-2gb.raw",
            &[0, 1, 2],
            "test-assets/fat32-2gb",
        );
    }

    fn test_mkd(input_file: &str, preload_ranges: &[u32], output_dir: &str) {
        if Path::new(output_dir).exists() {
            remove_dir_all(output_dir).unwrap();
        }

        let args = vec![
            "sockdrive".to_string(),
            "mkd".to_string(),
            input_file.to_string(),
            preload_ranges
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .join(","),
            output_dir.to_string(),
        ];

        mkd(args);

        let raw_path = Path::new(output_dir).join("1.raw");
        assert!(!raw_path.exists(), "1.raw file should not exist");

        let config_path = Path::new(output_dir).join("sockdrive.metaj");
        assert!(config_path.exists(), "sockdrive.metaj should exist");

        let config_str = std::fs::read_to_string(config_path).unwrap();
        let config: serde_json::Value = serde_json::from_str(&config_str).unwrap();

        let preload_ranges = config
            .get("preload_ranges")
            .expect("sockdrive.metaj should have preload_ranges field")
            .as_array()
            .expect("preload_ranges should be an array");

        assert_eq!(
            preload_ranges
                .iter()
                .map(|v| v.as_u64().unwrap())
                .collect::<Vec<_>>(),
            preload_ranges.to_vec(),
            "preload_ranges should be {:?}",
            preload_ranges
        );
    }

    #[test]
    fn restore_finds_sockdrive_mounts() {
        let mounts = find_sockdrive_mounts(
            "
[autoexec]
imgmount 2 sockdrive https://example.test/drive/
imgmount d cdrom.iso -t iso
imgmount c sockdrive wss://sockdrive.js-dos.com:8001/system/win95
",
        );

        assert_eq!(mounts.len(), 2);
        assert_eq!(mounts[0].drive, "2");
        assert_eq!(mounts[0].url, "https://example.test/drive");
        assert_eq!(
            mounts[1].url,
            "https://br.cdn.dos.zone/sockdrive-qcow2/system-win95"
        );
    }

    #[test]
    fn restore_parses_sockdrive_changes() {
        let url = "https://example.test/drive";
        let persist = [1u8, 2, 3, 4, 5];
        let mut encoded = Vec::new();
        encoded.extend_from_slice(&(url.len() as u32).to_le_bytes());
        encoded.extend_from_slice(url.as_bytes());
        encoded.extend_from_slice(&(persist.len() as u32).to_le_bytes());
        encoded.extend_from_slice(&persist);

        let changes = parse_sockdrive_changes(&encoded).unwrap();
        assert_eq!(changes.get(url).unwrap(), &persist);
    }

    #[test]
    fn restore_applies_uncompressed_sector_changes() {
        let dir =
            std::env::temp_dir().join(format!("sockdrive-restore-test-{}", std::process::id()));
        if dir.exists() {
            remove_dir_all(&dir).unwrap();
        }
        create_dir_all(&dir).unwrap();
        let raw_path = dir.join("disk.raw");
        write(&raw_path, vec![0u8; 1024]).unwrap();

        let sector_data = vec![0x7bu8; 512];
        let mut chunk = Vec::new();
        chunk.extend_from_slice(&1u32.to_le_bytes());
        chunk.extend_from_slice(&sector_data);

        let mut changes = Vec::new();
        changes.extend_from_slice(&1u32.to_le_bytes());
        changes.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
        changes.extend_from_slice(&chunk);

        let meta = SockdriveMeta {
            size_kb: 1,
            ahead_read: 1024,
            range_count: 1,
            sector_size: 512,
            dropped_ranges: Vec::new(),
            small_ranges: Vec::new(),
        };

        apply_sockdrive_changes(&raw_path, &changes, &meta).unwrap();
        let raw = std::fs::read(&raw_path).unwrap();
        assert_eq!(&raw[..512], vec![0u8; 512]);
        assert_eq!(&raw[512..], sector_data.as_slice());

        remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn doctor_output_dir_uses_bundle_stem_or_doctor_suffix() {
        assert_eq!(
            doctor_output_dir(Path::new("/tmp/game.jsdos")),
            Path::new("/tmp/game")
        );
        let dir = std::env::temp_dir();
        assert_eq!(
            doctor_output_dir(&dir),
            dir.with_file_name(format!(
                "{}.doctor",
                dir.file_name().unwrap().to_string_lossy()
            ))
        );
    }

    #[test]
    fn fat32_parser_lists_and_resolves_lfn_fragmented_file() {
        let dir = test_temp_dir("fat32-parser");
        let raw_path = dir.join("disk.raw");
        let file_data = vec![0x33u8; 600];
        write(&raw_path, create_test_fat32_image(&file_data)).unwrap();

        let mut raw = File::open(&raw_path).unwrap();
        let fat = read_fat32_info(&mut raw).unwrap();
        let entries = list_fat32_path(&mut raw, &fat, "/").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "Long File.txt");

        let entry = resolve_fat32_path(&mut raw, &fat, "long file.txt")
            .unwrap()
            .unwrap();
        assert_eq!(entry.size, 600);
        let extents = file_data_extents(&mut raw, &fat, &entry).unwrap();
        assert_eq!(extents, vec![(2560, 512), (3072, 88)]);

        remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn doctor_patch_updates_plain_ranges() {
        let dir = test_temp_dir("doctor-patch-plain");
        let file_data = vec![0x11u8; 600];
        write_test_doctor_mount(&dir, "2", &create_test_fat32_image(&file_data), &[], &[]);
        let replacement = vec![0x44u8; 600];
        let replacement_path = dir.join("replacement.bin");
        write(&replacement_path, &replacement).unwrap();

        doctor_patch_file(&dir, "2", "/Long File.txt", &replacement_path).unwrap();

        let restored = restore_test_doctor_raw(&dir, "2");
        assert_eq!(&restored[2560..3072], &replacement[..512]);
        assert_eq!(&restored[3072..3160], &replacement[512..]);

        remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn doctor_extract_writes_whole_file() {
        let dir = test_temp_dir("doctor-extract");
        let file_data = vec![0x22u8; 600];
        write_test_doctor_mount(&dir, "2", &create_test_fat32_image(&file_data), &[], &[]);
        let output_path = dir.join("extracted.bin");

        doctor_extract_file_to(&dir, "2", "/Long File.txt", &output_path).unwrap();

        assert_eq!(std::fs::read(output_path).unwrap(), file_data);
        remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn doctor_patch_updates_small_ranges_preload() {
        let dir = test_temp_dir("doctor-patch-small");
        let file_data = vec![0x11u8; 600];
        write_test_doctor_mount(&dir, "2", &create_test_fat32_image(&file_data), &[5], &[]);
        let replacement = vec![0x55u8; 600];
        let replacement_path = dir.join("replacement.bin");
        write(&replacement_path, &replacement).unwrap();

        doctor_patch_file(&dir, "2", "Long File.txt", &replacement_path).unwrap();

        let preload = std::fs::read(dir.join("2-test-drive/preload.raw")).unwrap();
        assert_eq!(preload, replacement[..512]);
        let restored = restore_test_doctor_raw(&dir, "2");
        assert_eq!(&restored[2560..3072], &replacement[..512]);
        assert_eq!(&restored[3072..3160], &replacement[512..]);

        remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn doctor_patch_resurrects_dropped_range() {
        let dir = test_temp_dir("doctor-patch-dropped");
        let file_data = vec![0x11u8; 600];
        write_test_doctor_mount(&dir, "2", &create_test_fat32_image(&file_data), &[], &[5]);
        let replacement = vec![0x66u8; 600];
        let replacement_path = dir.join("replacement.bin");
        write(&replacement_path, &replacement).unwrap();

        doctor_patch_file(&dir, "2", "Long File.txt", &replacement_path).unwrap();

        assert!(dir.join("2-test-drive/5.raw").exists());
        let meta_text = std::fs::read_to_string(dir.join("2-test-drive/sockdrive.metaj")).unwrap();
        let meta: serde_json::Value = serde_json::from_str(&meta_text).unwrap();
        assert_eq!(
            meta.get("dropped_ranges").unwrap().as_array().unwrap(),
            &Vec::<serde_json::Value>::new()
        );
        let restored = restore_test_doctor_raw(&dir, "2");
        assert_eq!(&restored[2560..3072], &replacement[..512]);

        remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn doctor_patch_size_mismatch_keeps_ranges_unchanged() {
        let dir = test_temp_dir("doctor-patch-size-mismatch");
        let file_data = vec![0x11u8; 600];
        write_test_doctor_mount(&dir, "2", &create_test_fat32_image(&file_data), &[], &[]);
        let before = std::fs::read(dir.join("2-test-drive/5.raw")).unwrap();
        let replacement_path = dir.join("replacement.bin");
        write(&replacement_path, vec![0x77u8; 599]).unwrap();

        let error = doctor_patch_file(&dir, "2", "Long File.txt", &replacement_path).unwrap_err();
        assert!(error.contains("replacement size mismatch"));
        let after = std::fs::read(dir.join("2-test-drive/5.raw")).unwrap();
        assert_eq!(before, after);

        remove_dir_all(&dir).unwrap();
    }

    fn test_temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sockdrive-{}-{}", name, std::process::id()));
        if dir.exists() {
            remove_dir_all(&dir).unwrap();
        }
        create_dir_all(&dir).unwrap();
        dir
    }

    fn write_test_doctor_mount(
        doctor_dir: &Path,
        drive: &str,
        image: &[u8],
        small_ranges: &[u32],
        dropped_ranges: &[u32],
    ) {
        let mount_dir = doctor_dir.join(format!("{}-test-drive", drive));
        create_dir_all(&mount_dir).unwrap();
        let ahead_read = 512usize;
        let range_count = image.len().div_ceil(ahead_read);
        let meta = serde_json::json!({
            "name": "test",
            "size": image.len() / 1024,
            "ahead_read": ahead_read,
            "range_count": range_count,
            "sector_size": 512,
            "dropped_ranges": dropped_ranges,
            "small_ranges": small_ranges,
        });
        write(
            mount_dir.join("sockdrive.metaj"),
            serde_json::to_string(&meta).unwrap(),
        )
        .unwrap();

        let small_set: HashSet<u32> = small_ranges.iter().copied().collect();
        let dropped_set: HashSet<u32> = dropped_ranges.iter().copied().collect();
        let mut preload = Vec::new();
        for range in 0..range_count {
            let start = range * ahead_read;
            let data = &image[start..start + ahead_read];
            let range_u32 = range as u32;
            if dropped_set.contains(&range_u32) {
                continue;
            }
            if small_set.contains(&range_u32) {
                preload.extend_from_slice(data);
            } else {
                write(mount_dir.join(format!("{}.raw", range)), data).unwrap();
            }
        }
        if !small_ranges.is_empty() {
            write(mount_dir.join("preload.raw"), preload).unwrap();
        }

        write_doctor_manifest(
            doctor_dir,
            "test.jsdos",
            &[DoctorMount {
                drive: drive.to_string(),
                url: "https://example.test/test-drive".to_string(),
                dir: format!("{}-test-drive", drive),
            }],
        )
        .unwrap();
    }

    fn restore_test_doctor_raw(doctor_dir: &Path, drive: &str) -> Vec<u8> {
        let mount = read_doctor_mount(doctor_dir, drive).unwrap();
        let raw_path = doctor_dir.join("restored.raw");
        restore_sockdrive_raw_from_dir(&doctor_dir.join(mount.dir), &raw_path).unwrap();
        std::fs::read(raw_path).unwrap()
    }

    fn create_test_fat32_image(file_data: &[u8]) -> Vec<u8> {
        assert!(file_data.len() > 512 && file_data.len() <= 1024);
        let mut image = vec![0u8; 8192];
        put_partition(&mut image);
        put_boot_sector(&mut image[512..1024]);
        put_fat(&mut image[1024..1536]);
        put_fat(&mut image[1536..2048]);
        put_root_dir(&mut image[2048..2560], file_data.len() as u32);
        image[2560..3072].copy_from_slice(&file_data[..512]);
        image[3072..3072 + file_data.len() - 512].copy_from_slice(&file_data[512..]);
        image
    }

    fn put_partition(image: &mut [u8]) {
        image[446] = 0x80;
        image[450] = 0x0b;
        put_u32(image, 454, 1);
        put_u32(image, 458, 15);
        image[510] = 0x55;
        image[511] = 0xaa;
    }

    fn put_boot_sector(sector: &mut [u8]) {
        sector[0] = 0xeb;
        sector[1] = 0x58;
        sector[2] = 0x90;
        sector[3..11].copy_from_slice(b"MSWIN4.1");
        put_u16(sector, 11, 512);
        sector[13] = 1;
        put_u16(sector, 14, 1);
        sector[16] = 2;
        sector[21] = 0xf8;
        put_u32(sector, 28, 1);
        put_u32(sector, 32, 15);
        put_u32(sector, 36, 1);
        put_u32(sector, 44, 2);
        sector[82..90].copy_from_slice(b"FAT32   ");
        sector[510] = 0x55;
        sector[511] = 0xaa;
    }

    fn put_fat(fat: &mut [u8]) {
        put_u32(fat, 0, 0x0fff_fff8);
        put_u32(fat, 4, 0xffff_ffff);
        put_u32(fat, 8, 0x0fff_ffff);
        put_u32(fat, 12, 4);
        put_u32(fat, 16, 0x0fff_ffff);
    }

    fn put_root_dir(root: &mut [u8], file_size: u32) {
        put_lfn_entry(&mut root[0..32], "Long File.txt");
        root[32..43].copy_from_slice(b"LONGFI~1TXT");
        root[43] = 0x20;
        put_u16(root, 32 + 20, 0);
        put_u16(root, 32 + 26, 3);
        put_u32(root, 32 + 28, file_size);
    }

    fn put_lfn_entry(entry: &mut [u8], name: &str) {
        entry.fill(0xff);
        entry[0] = 0x41;
        entry[11] = 0x0f;
        entry[12] = 0;
        entry[13] = 0;
        entry[26] = 0;
        entry[27] = 0;
        let offsets = [1usize, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
        let chars: Vec<u16> = name.encode_utf16().collect();
        for (index, offset) in offsets.iter().enumerate() {
            let value = if index < chars.len() {
                chars[index]
            } else if index == chars.len() {
                0
            } else {
                0xffff
            };
            put_u16(entry, *offset, value);
        }
    }

    fn put_u16(data: &mut [u8], offset: usize, value: u16) {
        data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }

    fn put_u32(data: &mut [u8], offset: usize, value: u32) {
        data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
}
