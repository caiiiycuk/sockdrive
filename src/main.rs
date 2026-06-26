use std::cmp::min;
use std::collections::{HashMap, HashSet};
use std::fs::{create_dir, create_dir_all, metadata, read_dir, remove_file, rename, write, File};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

const AHEAD_READ_SIZE: u64 = 256 * 1024;
const FAT16_256MB: &str = include_str!("../drives/fat16-256mb.json");
const FAT32_2GB: &str = include_str!("../drives/fat32-2gb.json");
const SMALL_FILES_THRESHOLD: u64 = 1024 * 1024;
const DEFAULT_PRELOAD: &str = "0,16,1,52,50,68,51,145,280,152,291,227,234,226,207,279,7195,257,233,179,231,390,177,346,66,71,96,197,297,70,90,113,146,87,89,98,2,93,199,236,54,198,129,228,296,299,311,180,20,100,208,218,219,232,276,300,24,114,143,195,229,239,253,241,277,289,49,155,240,21,23,99,116,151,217,97,202,429,32,157,262,327,200,201,25,156,237,278,329,82,141,142,154,158,178,338,339,84,78,65,148,160,271,282,117,119,144,275,83,85,92,3,159,242,274,105,118,543,64,187,261,269,86,225,545,22,38,57,188,287,330,176,359,544,56,281,295,245,79,30,407,165,194,235,285,465,101,238,411,58,138,193,293,394,133,134,168,412,6,55,62,163,333,343,112,172,428,430,17,18,19,67,184,332,171,104,7,36,284,334,386,395,139,167,357,431,37,76,140,460,244,258,331,532,290,12,13,14,69,153,272,328,396,461,675,175,663,664,149,405,531,15,123,63,464,53,31,221,252,294,340,344,35,60,72,288,246,459,462,463,4,513,546,677,9,75,94,164,216,251,363,220,658,701,397,59,196,230,354,364,667,323,533,729";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 || (args[1] != "mkd" && args[1] != "sockify" && args[1] != "restore") {
        eprintln!(
            "
sockdrive cl

Use one of the following commands:
    sockdrive mkd   - make sockdrive from raw / qcow2 image
    sockdrive sockify - transform jsdos bundle with qcow2 images to use sockdrive
    sockdrive restore - restore sockdrive mounts from jsdos bundle to qcow2 images
            "
        );
    } else if args[1] == "mkd" {
        mkd(args);
    } else if args[1] == "sockify" {
        sockify(args);
    } else if args[1] == "restore" {
        restore(args);
    }
}

#[derive(Clone, Debug)]
struct SockdriveMount {
    drive: String,
    url: String,
}

#[derive(Clone, Debug)]
struct SockdriveMeta {
    size_kb: u64,
    ahead_read: u64,
    range_count: u64,
    sector_size: u64,
    dropped_ranges: Vec<u32>,
    small_ranges: Vec<u32>,
}

fn restore(args: Vec<String>) {
    if args.len() < 4 || args.len() > 5 || args[1] != "restore" {
        eprintln!(
            "
sockdrive cli: restore

Usage:
    sockdrive restore <jsdos_bundle> <output_dir> [changes.bin]

    jsdos_bundle: path to a jsdos bundle containing sockdrive mounts
    output_dir: directory where restored qcow2 images will be placed, must not exist
    changes.bin: optional sockdrive changes file exported by js-dos

Example:
    sockdrive restore bundle.jsdos ./restored changes.bin
        "
        );
        std::process::exit(1);
    }

    let jsdos_bundle = &args[2];
    let output_dir = Path::new(&args[3]);
    let changes_file = args.get(4);

    if !Path::new(jsdos_bundle).exists() {
        eprintln!("Error: jsdos bundle '{}' does not exist", jsdos_bundle);
        std::process::exit(1);
    }

    if output_dir.exists() {
        eprintln!("Error: output directory '{}' exists", output_dir.display());
        std::process::exit(1);
    }

    let changes = match changes_file {
        Some(path) => {
            if !Path::new(path).exists() {
                eprintln!("Error: changes file '{}' does not exist", path);
                std::process::exit(1);
            }
            parse_sockdrive_changes(&std::fs::read(path).unwrap_or_else(|e| {
                eprintln!("Error: failed to read changes file '{}': {}", path, e);
                std::process::exit(1);
            }))
            .unwrap_or_else(|e| {
                eprintln!("Error: invalid changes file '{}': {}", path, e);
                std::process::exit(1);
            })
        }
        None => HashMap::new(),
    };

    create_dir_all(output_dir).unwrap();
    let temp_dir = output_dir.join("sockdrive-restore-temp");
    create_dir_all(&temp_dir).unwrap();

    let cleanup = || {
        if temp_dir.exists() {
            std::fs::remove_dir_all(&temp_dir).unwrap();
        }
    };

    if Path::new(jsdos_bundle).is_dir() {
        copy_dir_all(jsdos_bundle, &temp_dir).unwrap();
    } else {
        let status = Command::new("7z")
            .args(["x", jsdos_bundle, &format!("-o{}", temp_dir.display())])
            .status()
            .expect("Failed to run 7z");
        if !status.success() {
            eprintln!("Error: failed to extract jsdos bundle '{}'", jsdos_bundle);
            cleanup();
            std::process::exit(1);
        }
    }

    let dosbox_conf = temp_dir.join(".jsdos/dosbox.conf");
    if !dosbox_conf.exists() {
        eprintln!(
            "Error: dosbox.conf '{}' does not exist",
            dosbox_conf.display()
        );
        cleanup();
        std::process::exit(1);
    }

    let dosbox_conf_content = std::fs::read_to_string(&dosbox_conf).unwrap();
    let mounts = find_sockdrive_mounts(&dosbox_conf_content);
    if mounts.is_empty() {
        eprintln!("Error: sockdrive mounts not found in dosbox.conf");
        cleanup();
        std::process::exit(1);
    }

    copy_bundle_payload(&temp_dir, output_dir).unwrap_or_else(|e| {
        eprintln!(
            "Error: failed to copy jsdos bundle payload to '{}': {}",
            output_dir.display(),
            e
        );
        cleanup();
        std::process::exit(1);
    });

    let mut used_names = HashSet::new();
    let mut restored_mounts = Vec::new();
    for (index, mount) in mounts.iter().enumerate() {
        let basename = unique_restore_name(index, mount, &mut used_names);
        let qcow2_name = format!("{}.qcow2", basename);
        let drive_temp_dir = temp_dir.join(format!("drive-{}", index));
        create_dir_all(&drive_temp_dir).unwrap();
        let raw_path = drive_temp_dir.join(format!("{}.raw", basename));
        let qcow2_path = output_dir.join(&qcow2_name);

        println!("Restoring {} to {}", mount.url, qcow2_path.display());
        restore_sockdrive_raw(
            &mount.url,
            &raw_path,
            &drive_temp_dir,
            changes.get(&mount.url).map(|v| v.as_slice()),
        )
        .unwrap_or_else(|e| {
            eprintln!("Error: failed to restore '{}': {}", mount.url, e);
            cleanup();
            std::process::exit(1);
        });

        convert_raw_to_qcow2(&raw_path, &qcow2_path).unwrap_or_else(|e| {
            eprintln!("Error: failed to convert '{}': {}", raw_path.display(), e);
            cleanup();
            std::process::exit(1);
        });

        run_qemu_scandisk(&qcow2_path).unwrap_or_else(|e| {
            eprintln!(
                "Error: qemu scandisk failed for '{}': {}",
                qcow2_path.display(),
                e
            );
            cleanup();
            std::process::exit(1);
        });

        restored_mounts.push((mount.clone(), qcow2_name));
    }

    for url in changes.keys() {
        if !mounts.iter().any(|mount| &mount.url == url) {
            eprintln!("Warning: changes for '{}' were not used", url);
        }
    }

    let restored_dosbox_conf = replace_sockdrive_mounts(&dosbox_conf_content, &restored_mounts)
        .unwrap_or_else(|e| {
            eprintln!("Error: failed to rewrite dosbox.conf: {}", e);
            cleanup();
            std::process::exit(1);
        });
    let output_dosbox_conf = output_dir.join(".jsdos/dosbox.conf");
    std::fs::write(&output_dosbox_conf, restored_dosbox_conf).unwrap_or_else(|e| {
        eprintln!(
            "Error: failed to write '{}': {}",
            output_dosbox_conf.display(),
            e
        );
        cleanup();
        std::process::exit(1);
    });

    cleanup();
    let restored_bundle = pack_restored_jsdos(jsdos_bundle, output_dir, &restored_mounts)
        .unwrap_or_else(|e| {
            eprintln!("Error: failed to pack restored jsdos bundle: {}", e);
            std::process::exit(1);
        });
    println!(
        "Done, restored {} qcow2 image(s) in {}, bundle {}",
        mounts.len(),
        output_dir.display(),
        restored_bundle.display()
    );
}

fn copy_bundle_payload(src: &Path, dst: &Path) -> Result<(), String> {
    for entry in read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let target = dst.join(entry.file_name());
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            copy_dir_all(entry.path(), target).map_err(|e| e.to_string())?;
        } else {
            std::fs::copy(entry.path(), target).map_err(|e| e.to_string())?;
        }
    }

    Ok(())
}

fn replace_sockdrive_mounts(
    dosbox_conf: &str,
    restored_mounts: &[(SockdriveMount, String)],
) -> Result<String, String> {
    let mut used = vec![false; restored_mounts.len()];
    let mut replaced = 0usize;
    let mut lines = Vec::new();

    for line in dosbox_conf.lines() {
        let trimmed = line.trim_start();
        let indent_len = line.len() - trimmed.len();
        let indent = &line[..indent_len];
        let parts: Vec<&str> = trimmed.split_whitespace().collect();

        if parts.len() >= 4 && parts[0] == "imgmount" && parts[2] == "sockdrive" {
            let drive = parts[1];
            let url = normalize_sockdrive_url(parts[3]);
            if let Some((index, (_, qcow2_name))) =
                restored_mounts
                    .iter()
                    .enumerate()
                    .find(|(index, (mount, _))| {
                        !used[*index] && mount.drive == drive && mount.url == url
                    })
            {
                used[index] = true;
                replaced += 1;
                lines.push(format!("{}imgmount {} {}", indent, drive, qcow2_name));
                continue;
            }
        }

        lines.push(line.to_string());
    }

    if replaced != restored_mounts.len() {
        return Err(format!(
            "replaced {} sockdrive mount(s), expected {}",
            replaced,
            restored_mounts.len()
        ));
    }

    Ok(lines.join("\n"))
}

fn pack_restored_jsdos(
    jsdos_bundle: &str,
    output_dir: &Path,
    restored_mounts: &[(SockdriveMount, String)],
) -> Result<PathBuf, String> {
    let source_name = Path::new(jsdos_bundle)
        .file_stem()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("restored");
    let archive_name = format!("{}.restored.jsdos", source_name);
    let archive_path = output_dir.join(&archive_name);
    if archive_path.exists() {
        return Err(format!(
            "archive '{}' already exists",
            archive_path.display()
        ));
    }

    let mut command = Command::new("7z");
    command.current_dir(output_dir);
    command.args(["a", "-tzip", "-mx0", &archive_name, ".jsdos"]);
    for (_, qcow2_name) in restored_mounts {
        command.arg(qcow2_name);
    }

    let status = command
        .status()
        .map_err(|e| format!("failed to run 7z: {}", e))?;
    if !status.success() {
        return Err("7z failed".to_string());
    }

    Ok(archive_path)
}

fn find_sockdrive_mounts(dosbox_conf: &str) -> Vec<SockdriveMount> {
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

fn normalize_sockdrive_url(url: &str) -> String {
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

fn unique_restore_name(
    index: usize,
    mount: &SockdriveMount,
    used_names: &mut HashSet<String>,
) -> String {
    let raw_name = mount
        .url
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("sockdrive");
    let mut name = sanitize_filename(raw_name);
    if name.is_empty() {
        name = format!("sockdrive-{}", index);
    }

    if used_names.insert(name.clone()) {
        return name;
    }

    let mut candidate = format!("{}-{}", mount.drive, name);
    candidate = sanitize_filename(&candidate);
    if used_names.insert(candidate.clone()) {
        return candidate;
    }

    let candidate = format!("{}-{}", candidate, index);
    used_names.insert(candidate.clone());
    candidate
}

fn sanitize_filename(name: &str) -> String {
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

fn restore_sockdrive_raw(
    url: &str,
    raw_path: &Path,
    temp_dir: &Path,
    changes: Option<&[u8]>,
) -> Result<(), String> {
    let meta_path = temp_dir.join("sockdrive.metaj.download");
    download_sockdrive_file(url, "sockdrive.metaj", &meta_path)?;
    let meta_bytes = decode_downloaded_json(&meta_path)?;
    let meta: serde_json::Value = serde_json::from_slice(&meta_bytes)
        .map_err(|e| format!("invalid sockdrive.metaj JSON: {}", e))?;
    let meta = parse_sockdrive_meta(&meta)?;
    validate_sockdrive_meta(&meta)?;

    let raw_size = meta.size_kb * 1024;
    let mut raw = File::create(raw_path).map_err(|e| e.to_string())?;
    raw.set_len(raw_size).map_err(|e| e.to_string())?;

    let dropped: HashSet<u32> = meta.dropped_ranges.iter().copied().collect();
    let small: HashSet<u32> = meta.small_ranges.iter().copied().collect();
    let preload = if meta.small_ranges.is_empty() {
        Vec::new()
    } else {
        let preload_path = temp_dir.join("preload.raw.download");
        download_sockdrive_file(url, "preload.raw", &preload_path)?;
        decode_downloaded_len(
            &preload_path,
            meta.small_ranges.len() * meta.ahead_read as usize,
        )?
    };

    let mut normal_ranges = Vec::new();
    for range in 0..meta.range_count {
        let range_u32 = range as u32;
        let offset = range * meta.ahead_read;
        if offset >= raw_size || dropped.contains(&range_u32) {
            continue;
        }

        let source =
            if let Some(small_index) = meta.small_ranges.iter().position(|r| *r == range_u32) {
                let start = small_index * meta.ahead_read as usize;
                let end = start + meta.ahead_read as usize;
                &preload[start..end]
            } else if small.contains(&range_u32) {
                return Err(format!("small range {} is duplicated", range));
            } else {
                normal_ranges.push(range);
                continue;
            };

        let write_len = min(meta.ahead_read, raw_size - offset) as usize;
        raw.seek(std::io::SeekFrom::Start(offset))
            .map_err(|e| e.to_string())?;
        raw.write_all(&source[..write_len])
            .map_err(|e| e.to_string())?;
    }

    restore_sockdrive_ranges_parallel(
        url,
        raw,
        temp_dir,
        &normal_ranges,
        meta.ahead_read,
        raw_size,
    )?;

    if let Some(changes) = changes {
        apply_sockdrive_changes(raw_path, changes, &meta)?;
    }

    Ok(())
}

fn restore_sockdrive_ranges_parallel(
    url: &str,
    raw: File,
    temp_dir: &Path,
    ranges: &[u64],
    ahead_read: u64,
    raw_size: u64,
) -> Result<(), String> {
    if ranges.is_empty() {
        return Ok(());
    }

    let workers = min(8, ranges.len());
    let queue = Arc::new(Mutex::new(std::collections::VecDeque::from(
        ranges.to_vec(),
    )));
    let raw = Arc::new(Mutex::new(raw));
    let error = Arc::new(Mutex::new(None::<String>));
    let completed = Arc::new(AtomicUsize::new(0));
    let total = ranges.len();
    let mut handles = Vec::new();

    eprintln!(
        "Downloading {} sockdrive ranges with {} workers",
        total, workers
    );

    for _ in 0..workers {
        let url = url.to_string();
        let temp_dir = temp_dir.to_path_buf();
        let queue = Arc::clone(&queue);
        let raw = Arc::clone(&raw);
        let error = Arc::clone(&error);
        let completed = Arc::clone(&completed);

        handles.push(thread::spawn(move || loop {
            if error.lock().unwrap().is_some() {
                break;
            }

            let Some(range) = queue.lock().unwrap().pop_front() else {
                break;
            };

            if let Err(e) =
                restore_sockdrive_range(&url, &temp_dir, &raw, range, ahead_read, raw_size)
            {
                *error.lock().unwrap() = Some(e);
                break;
            }

            let done = completed.fetch_add(1, Ordering::SeqCst) + 1;
            if done == total || done % 100 == 0 {
                eprintln!("Restored {}/{} sockdrive ranges", done, total);
            }
        }));
    }

    for handle in handles {
        handle
            .join()
            .map_err(|_| "sockdrive range worker panicked".to_string())?;
    }

    if let Some(error) = error.lock().unwrap().take() {
        return Err(error);
    }

    Ok(())
}

fn restore_sockdrive_range(
    url: &str,
    temp_dir: &Path,
    raw: &Arc<Mutex<File>>,
    range: u64,
    ahead_read: u64,
    raw_size: u64,
) -> Result<(), String> {
    let range_path = temp_dir.join(format!("{}.raw.download", range));
    download_sockdrive_file(url, &format!("{}.raw", range), &range_path)?;
    let data = decode_downloaded_len(&range_path, ahead_read as usize)?;
    let offset = range * ahead_read;
    let write_len = min(ahead_read, raw_size - offset) as usize;

    let mut raw = raw.lock().unwrap();
    raw.seek(std::io::SeekFrom::Start(offset))
        .map_err(|e| e.to_string())?;
    raw.write_all(&data[..write_len])
        .map_err(|e| e.to_string())?;

    Ok(())
}

fn parse_sockdrive_meta(meta: &serde_json::Value) -> Result<SockdriveMeta, String> {
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

fn optional_u32_array(meta: &serde_json::Value, field: &str) -> Result<Vec<u32>, String> {
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

fn validate_sockdrive_meta(meta: &SockdriveMeta) -> Result<(), String> {
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

fn download_sockdrive_file(url: &str, name: &str, output: &Path) -> Result<(), String> {
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

fn decode_downloaded_json(path: &Path) -> Result<Vec<u8>, String> {
    for candidate in decode_candidates(path)? {
        if serde_json::from_slice::<serde_json::Value>(&candidate).is_ok() {
            return Ok(candidate);
        }
    }

    Err(format!(
        "downloaded file '{}' is not valid JSON",
        path.display()
    ))
}

fn decode_downloaded_len(path: &Path, expected_len: usize) -> Result<Vec<u8>, String> {
    for candidate in decode_candidates(path)? {
        if candidate.len() == expected_len {
            return Ok(candidate);
        }
    }

    Err(format!(
        "downloaded file '{}' does not decode to expected size {}",
        path.display(),
        expected_len
    ))
}

fn decode_candidates(path: &Path) -> Result<Vec<Vec<u8>>, String> {
    let original = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut candidates = vec![original];

    if let Some(decoded) = decode_with_command("gzip", &["-d", "-c"], path)? {
        candidates.push(decoded);
    }

    if let Some(decoded) = decode_with_command("brotli", &["-d", "-c"], path)? {
        candidates.push(decoded);
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

fn parse_sockdrive_changes(encoded: &[u8]) -> Result<HashMap<String, Vec<u8>>, String> {
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

fn apply_sockdrive_changes(
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

fn convert_raw_to_qcow2(raw_path: &Path, qcow2_path: &Path) -> Result<(), String> {
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

fn run_qemu_scandisk(image_path: &Path) -> Result<(), String> {
    let boot_img = find_boot_img()?;
    let floppy_drive = format!("file={},format=raw,if=floppy", boot_img.display());
    let hda_drive = format!("file={},format=qcow2,if=ide", image_path.display());
    let status = Command::new("qemu-system-i386")
        .args([
            "-boot",
            "a",
            "-drive",
            &floppy_drive,
            "-drive",
            &hda_drive,
            "-display",
            "none",
            "--no-reboot",
        ])
        .status()
        .map_err(|e| format!("failed to run qemu-system-i386: {}", e))?;

    if !status.success() {
        return Err("qemu-system-i386 failed".to_string());
    }

    Ok(())
}

fn find_boot_img() -> Result<PathBuf, String> {
    let exe_boot_img = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .parent()
        .ok_or_else(|| "failed to find executable directory".to_string())?
        .join("boot.img");
    if exe_boot_img.exists() {
        return Ok(exe_boot_img);
    }

    let cwd_boot_img = Path::new("etc/boot.img").to_path_buf();
    if cwd_boot_img.exists() {
        return Ok(cwd_boot_img);
    }

    Err(format!(
        "boot.img '{}' does not exist",
        exe_boot_img.display()
    ))
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

    let output_dir = if output_dir.starts_with("./") {
        output_dir[2..].to_string()
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

fn copy_dir_all(
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
    use std::fs::remove_dir_all;
    use std::path::Path;

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
    fn restore_rewrites_sockdrive_mounts_to_qcow2() {
        let conf = "[autoexec]\n  imgmount 2 sockdrive https://example.test/drive/\nboot -l c";
        let mounts = vec![(
            SockdriveMount {
                drive: "2".to_string(),
                url: "https://example.test/drive".to_string(),
            },
            "drive.qcow2".to_string(),
        )];

        let rewritten = replace_sockdrive_mounts(conf, &mounts).unwrap();

        assert_eq!(rewritten, "[autoexec]\n  imgmount 2 drive.qcow2\nboot -l c");
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
}
