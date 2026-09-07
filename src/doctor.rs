use crate::fat32::{
    file_data_extents, list_fat32_path, read_fat32_info, read_file_extents, resolve_fat32_path,
    write_replacement_to_raw,
};
use crate::{
    apply_sockdrive_changes, convert_raw_to_qcow2, copy_dir_all, decode_file_len, decode_json_file,
    download_sockdrive_file, encode_file, find_sockdrive_mounts, normalize_sockdrive_url,
    optional_u32_array, parse_sockdrive_changes, parse_sockdrive_meta, sanitize_filename,
    validate_sockdrive_meta, SockdriveMeta, SockdriveMount, StoredEncoding,
};
use std::cmp::min;
use std::collections::{HashMap, HashSet};
use std::fs::{create_dir_all, remove_file, File};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub(crate) struct DoctorMount {
    pub(crate) drive: String,
    pub(crate) url: String,
    pub(crate) dir: String,
}

pub(crate) fn doctor(args: Vec<String>) {
    if args.len() == 3
        && args[2] != "ls"
        && args[2] != "patch"
        && args[2] != "extract"
        && args[2] != "restore"
    {
        doctor_download(&args[2]);
        return;
    }

    if args.len() >= 3 && args[2] == "ls" {
        if args.len() != 5 && args.len() != 6 {
            doctor_usage();
            std::process::exit(1);
        }

        doctor_ls(
            Path::new(&args[3]),
            &args[4],
            args.get(5).map(String::as_str),
        )
        .unwrap_or_else(|e| {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        });
        return;
    }

    if args.len() >= 3 && args[2] == "extract" {
        if args.len() != 6 {
            doctor_usage();
            std::process::exit(1);
        }

        doctor_extract_file(Path::new(&args[3]), &args[4], &args[5]).unwrap_or_else(|e| {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        });
        return;
    }

    if args.len() >= 3 && args[2] == "restore" {
        let (doctor_dir, changes_file) = match args.len() {
            3 => (
                std::env::current_dir().unwrap_or_else(|e| {
                    eprintln!("Error: failed to get current directory: {}", e);
                    std::process::exit(1);
                }),
                None,
            ),
            4 => {
                let path = Path::new(&args[3]);
                let current_dir = std::env::current_dir().unwrap_or_else(|e| {
                    eprintln!("Error: failed to get current directory: {}", e);
                    std::process::exit(1);
                });
                if path.is_file() && current_dir.join("doctor.json").exists() {
                    (current_dir, Some(args[3].as_str()))
                } else {
                    (path.to_path_buf(), None)
                }
            }
            5 => (PathBuf::from(&args[3]), Some(args[4].as_str())),
            _ => {
                doctor_usage();
                std::process::exit(1);
            }
        };

        doctor_restore(&doctor_dir, changes_file).unwrap_or_else(|e| {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        });
        return;
    }

    if args.len() >= 3 && args[2] == "patch" {
        if args.len() != 7 {
            doctor_usage();
            std::process::exit(1);
        }

        doctor_patch_file(Path::new(&args[3]), &args[4], &args[5], Path::new(&args[6]))
            .unwrap_or_else(|e| {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            });
        return;
    }

    doctor_usage();
    std::process::exit(1);
}

fn doctor_usage() {
    eprintln!(
        "
sockdrive cli: doctor

Usage:
    sockdrive doctor <jsdos_bundle>
    sockdrive doctor ls <doctor_dir> <drive> [fat_path]
    sockdrive doctor extract <doctor_dir> <drive> <fat_path>
    sockdrive doctor patch <doctor_dir> <drive> <fat_path> <replacement_file>
    sockdrive doctor restore [doctor_dir] [changes.bin]
    sockdrive doctor restore <changes.bin>    # from inside doctor_dir

    jsdos_bundle: path to a jsdos bundle containing sockdrive mounts
    doctor_dir: directory created by `sockdrive doctor <jsdos_bundle>`
    drive: DOSBox imgmount drive id, for example 2 or 3
    fat_path: path inside the FAT32 filesystem
    extract writes to the current directory using the FAT32 file name
    restore writes qcow2 images into doctor_dir
        "
    );
}

fn doctor_download(jsdos_bundle: &str) {
    let bundle_path = Path::new(jsdos_bundle);
    if !bundle_path.exists() {
        eprintln!("Error: jsdos bundle '{}' does not exist", jsdos_bundle);
        std::process::exit(1);
    }

    let output_dir = doctor_output_dir(bundle_path);
    if output_dir.exists() {
        eprintln!("Error: doctor directory '{}' exists", output_dir.display());
        std::process::exit(1);
    }

    create_dir_all(&output_dir).unwrap();
    let temp_dir = output_dir.join("sockdrive-doctor-temp");
    create_dir_all(&temp_dir).unwrap();
    let cleanup = || {
        if temp_dir.exists() {
            std::fs::remove_dir_all(&temp_dir).unwrap();
        }
    };

    if bundle_path.is_dir() {
        copy_dir_all(bundle_path, &temp_dir).unwrap();
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

    let mut doctor_mounts = Vec::new();
    let mut used_dirs = HashSet::new();
    for (index, mount) in mounts.iter().enumerate() {
        let dir_name = unique_doctor_mount_dir(index, mount, &mut used_dirs);
        let mount_dir = output_dir.join(&dir_name);
        create_dir_all(&mount_dir).unwrap();
        println!("Downloading {} to {}", mount.url, mount_dir.display());
        download_sockdrive_mount(&mount.url, &mount_dir).unwrap_or_else(|e| {
            eprintln!("Error: failed to download '{}': {}", mount.url, e);
            cleanup();
            std::process::exit(1);
        });

        doctor_mounts.push(DoctorMount {
            drive: mount.drive.clone(),
            url: mount.url.clone(),
            dir: dir_name,
        });
    }

    write_doctor_manifest(&output_dir, jsdos_bundle, &doctor_mounts).unwrap_or_else(|e| {
        eprintln!("Error: failed to write doctor.json: {}", e);
        cleanup();
        std::process::exit(1);
    });

    cleanup();
    println!(
        "Done, downloaded {} sockdrive mount(s) to {}",
        doctor_mounts.len(),
        output_dir.display()
    );
}

pub(crate) fn doctor_output_dir(bundle_path: &Path) -> PathBuf {
    if bundle_path.is_dir() {
        let name = bundle_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("bundle");
        return bundle_path.with_file_name(format!("{}.doctor", name));
    }

    let stem = bundle_path
        .file_stem()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("bundle");
    bundle_path.with_file_name(stem)
}

fn unique_doctor_mount_dir(
    index: usize,
    mount: &SockdriveMount,
    used_dirs: &mut HashSet<String>,
) -> String {
    let raw_name = mount
        .url
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("sockdrive");
    let base = sanitize_filename(&format!("{}-{}", mount.drive, raw_name));
    if used_dirs.insert(base.clone()) {
        return base;
    }

    let candidate = sanitize_filename(&format!("{}-{}", base, index));
    used_dirs.insert(candidate.clone());
    candidate
}

fn download_sockdrive_mount(url: &str, mount_dir: &Path) -> Result<(), String> {
    let meta_path = mount_dir.join("sockdrive.metaj");
    download_sockdrive_file(url, "sockdrive.metaj", &meta_path)?;
    let (meta_value, meta, _) = read_sockdrive_meta_file(&meta_path)?;

    let dropped: HashSet<u32> = meta.dropped_ranges.iter().copied().collect();
    let small: HashSet<u32> = meta.small_ranges.iter().copied().collect();
    if !small.is_empty() {
        download_sockdrive_file(url, "preload.raw", &mount_dir.join("preload.raw"))?;
    }

    let raw_size = meta.size_kb * 1024;
    let mut normal_ranges = Vec::new();
    for range in 0..meta.range_count {
        let range_u32 = range as u32;
        if range * meta.ahead_read >= raw_size
            || dropped.contains(&range_u32)
            || small.contains(&range_u32)
        {
            continue;
        }
        normal_ranges.push(range);
    }

    download_sockdrive_ranges_to_dir_parallel(url, mount_dir, &normal_ranges)?;
    validate_sockdrive_meta_value(&meta_value, &meta)?;
    Ok(())
}

fn download_sockdrive_ranges_to_dir_parallel(
    url: &str,
    mount_dir: &Path,
    ranges: &[u64],
) -> Result<(), String> {
    if ranges.is_empty() {
        return Ok(());
    }

    let workers = min(8, ranges.len());
    let queue = Arc::new(Mutex::new(std::collections::VecDeque::from(
        ranges.to_vec(),
    )));
    let error = Arc::new(Mutex::new(None::<String>));
    let completed = Arc::new(AtomicUsize::new(0));
    let total = ranges.len();
    let mut handles = Vec::new();

    eprintln!(
        "Downloading {} sockdrive files with {} workers",
        total, workers
    );

    for _ in 0..workers {
        let url = url.to_string();
        let mount_dir = mount_dir.to_path_buf();
        let queue = Arc::clone(&queue);
        let error = Arc::clone(&error);
        let completed = Arc::clone(&completed);

        handles.push(thread::spawn(move || loop {
            if error.lock().unwrap().is_some() {
                break;
            }

            let Some(range) = queue.lock().unwrap().pop_front() else {
                break;
            };

            if let Err(e) = download_sockdrive_file(
                &url,
                &format!("{}.raw", range),
                &mount_dir.join(format!("{}.raw", range)),
            ) {
                *error.lock().unwrap() = Some(e);
                break;
            }

            let done = completed.fetch_add(1, Ordering::SeqCst) + 1;
            if done == total || done.is_multiple_of(100) {
                eprintln!("Downloaded {}/{} sockdrive files", done, total);
            }
        }));
    }

    for handle in handles {
        handle
            .join()
            .map_err(|_| "sockdrive download worker panicked".to_string())?;
    }

    if let Some(error) = error.lock().unwrap().take() {
        return Err(error);
    }

    Ok(())
}

fn validate_sockdrive_meta_value(
    meta_value: &serde_json::Value,
    meta: &SockdriveMeta,
) -> Result<(), String> {
    let preload_ranges = optional_u32_array(meta_value, "preload_ranges")?;
    for range in preload_ranges {
        if range as u64 >= meta.range_count {
            return Err(format!(
                "preload range {} is outside range_count {}",
                range, meta.range_count
            ));
        }
    }
    Ok(())
}

pub(crate) fn write_doctor_manifest(
    doctor_dir: &Path,
    source: &str,
    mounts: &[DoctorMount],
) -> Result<(), String> {
    let mounts_json: Vec<serde_json::Value> = mounts
        .iter()
        .map(|mount| {
            serde_json::json!({
                "drive": mount.drive,
                "url": mount.url,
                "dir": mount.dir,
            })
        })
        .collect();
    let manifest = serde_json::json!({
        "version": 1,
        "source": source,
        "mounts": mounts_json,
    });
    let text = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    std::fs::write(doctor_dir.join("doctor.json"), text).map_err(|e| e.to_string())
}

fn read_doctor_manifest(doctor_dir: &Path) -> Result<Vec<DoctorMount>, String> {
    let manifest_path = doctor_dir.join("doctor.json");
    let manifest_text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("failed to read '{}': {}", manifest_path.display(), e))?;
    let manifest: serde_json::Value =
        serde_json::from_str(&manifest_text).map_err(|e| format!("invalid doctor.json: {}", e))?;
    let mounts = manifest
        .get("mounts")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "doctor.json field 'mounts' is missing or invalid".to_string())?;

    mounts
        .iter()
        .map(|mount| {
            let drive = mount
                .get("drive")
                .and_then(|value| value.as_str())
                .ok_or_else(|| "doctor.json mount field 'drive' is missing".to_string())?;
            let dir = mount
                .get("dir")
                .and_then(|value| value.as_str())
                .ok_or_else(|| "doctor.json mount field 'dir' is missing".to_string())?;
            let url = mount
                .get("url")
                .and_then(|value| value.as_str())
                .ok_or_else(|| "doctor.json mount field 'url' is missing".to_string())?;
            Ok(DoctorMount {
                drive: drive.to_string(),
                url: normalize_sockdrive_url(url),
                dir: dir.to_string(),
            })
        })
        .collect()
}

pub(crate) fn read_doctor_mount(doctor_dir: &Path, drive: &str) -> Result<DoctorMount, String> {
    let mut found = Vec::new();
    for mount in read_doctor_manifest(doctor_dir)? {
        if mount.drive == drive {
            found.push(mount);
        }
    }

    match found.len() {
        0 => Err(format!("drive '{}' not found in doctor.json", drive)),
        1 => Ok(found.remove(0)),
        _ => Err(format!("drive '{}' is ambiguous in doctor.json", drive)),
    }
}

fn doctor_restore(doctor_dir: &Path, changes_file: Option<&str>) -> Result<(), String> {
    if !doctor_dir.is_dir() {
        return Err(format!(
            "doctor directory '{}' does not exist",
            doctor_dir.display()
        ));
    }

    let mut changes = match changes_file {
        Some(path) => {
            if !Path::new(path).exists() {
                return Err(format!("changes file '{}' does not exist", path));
            }
            parse_sockdrive_changes(
                &std::fs::read(path)
                    .map_err(|e| format!("failed to read changes file '{}': {}", path, e))?,
            )
            .map_err(|e| format!("invalid changes file '{}': {}", path, e))?
        }
        None => HashMap::new(),
    };

    let mounts = read_doctor_manifest(doctor_dir)?;
    if mounts.is_empty() {
        return Err("doctor.json contains no mounts".to_string());
    }

    let mut used_names = HashSet::new();
    for (index, mount) in mounts.iter().enumerate() {
        let basename = unique_restore_name(index, mount, &mut used_names);
        let qcow2_path = doctor_dir.join(format!("{}.qcow2", basename));
        if qcow2_path.exists() {
            return Err(format!("qcow2 image '{}' exists", qcow2_path.display()));
        }

        let mount_dir = doctor_dir.join(&mount.dir);
        let raw_path = temp_raw_path("restore", &mount.drive);
        println!(
            "Restoring drive {} to {}",
            mount.drive,
            qcow2_path.display()
        );

        let restore_result = (|| {
            restore_sockdrive_raw_from_dir(&mount_dir, &raw_path)?;
            if let Some(change) = changes.remove(&mount.url) {
                let (_, meta, _) = read_sockdrive_meta_file(&mount_dir.join("sockdrive.metaj"))?;
                apply_sockdrive_changes(&raw_path, &change, &meta)?;
            }

            convert_raw_to_qcow2(&raw_path, &qcow2_path)
        })();

        let _ = remove_file(&raw_path);
        if restore_result.is_err() {
            let _ = remove_file(&qcow2_path);
        }
        restore_result?;
    }

    for url in changes.keys() {
        eprintln!("Warning: changes for '{}' were not used", url);
    }

    println!(
        "Done, restored {} qcow2 image(s) in {}",
        mounts.len(),
        doctor_dir.display()
    );
    Ok(())
}

fn unique_restore_name(
    index: usize,
    mount: &DoctorMount,
    used_names: &mut HashSet<String>,
) -> String {
    let raw_name = mount
        .url
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(&mount.dir);
    let mut name = sanitize_filename(raw_name);
    if name.is_empty() {
        name = sanitize_filename(&mount.dir);
    }
    if name.is_empty() {
        name = format!("sockdrive-{}", index);
    }

    if used_names.insert(name.clone()) {
        return name;
    }

    let candidate = sanitize_filename(&format!("{}-{}", mount.drive, name));
    if used_names.insert(candidate.clone()) {
        return candidate;
    }

    let candidate = format!("{}-{}", candidate, index);
    used_names.insert(candidate.clone());
    candidate
}

fn doctor_ls(doctor_dir: &Path, drive: &str, fat_path: Option<&str>) -> Result<(), String> {
    let mount = read_doctor_mount(doctor_dir, drive)?;
    let mount_dir = doctor_dir.join(&mount.dir);
    let raw_path = temp_raw_path("ls", drive);
    restore_sockdrive_raw_from_dir(&mount_dir, &raw_path)?;

    let result = (|| {
        let mut raw = File::open(&raw_path).map_err(|e| e.to_string())?;
        let fat = read_fat32_info(&mut raw)?;
        let entries = list_fat32_path(&mut raw, &fat, fat_path.unwrap_or(""))?;
        for entry in entries {
            let kind = if entry.is_dir() { "dir " } else { "file" };
            println!(
                "{} {:>10} {:08x} {}",
                kind, entry.size, entry.first_cluster, entry.name
            );
        }
        Ok::<(), String>(())
    })();

    let _ = remove_file(&raw_path);
    result
}

fn doctor_extract_file(doctor_dir: &Path, drive: &str, fat_path: &str) -> Result<PathBuf, String> {
    let output_path = default_extract_output_path(fat_path)?;
    doctor_extract_file_to(doctor_dir, drive, fat_path, &output_path)?;
    println!("Extracted '{}' to {}", fat_path, output_path.display());
    Ok(output_path)
}

fn default_extract_output_path(fat_path: &str) -> Result<PathBuf, String> {
    let name = fat_path
        .trim_matches(|c| c == '/' || c == '\\')
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .ok_or_else(|| "extract path should point to a file".to_string())?;
    Ok(std::env::current_dir()
        .map_err(|e| e.to_string())?
        .join(name))
}

pub(crate) fn doctor_extract_file_to(
    doctor_dir: &Path,
    drive: &str,
    fat_path: &str,
    output_path: &Path,
) -> Result<(), String> {
    if output_path.exists() {
        return Err(format!("output file '{}' exists", output_path.display()));
    }

    let mount = read_doctor_mount(doctor_dir, drive)?;
    let mount_dir = doctor_dir.join(&mount.dir);
    let raw_path = temp_raw_path("extract", drive);
    restore_sockdrive_raw_from_dir(&mount_dir, &raw_path)?;

    let result = (|| {
        let mut raw = File::open(&raw_path).map_err(|e| e.to_string())?;
        let fat = read_fat32_info(&mut raw)?;
        let entry = resolve_fat32_path(&mut raw, &fat, fat_path)?
            .ok_or_else(|| format!("FAT32 file '{}' not found", fat_path))?;
        if entry.is_dir() {
            return Err(format!("FAT32 path '{}' is a directory", fat_path));
        }
        let extents = file_data_extents(&mut raw, &fat, &entry)?;
        let data = read_file_extents(&mut raw, &extents)?;
        std::fs::write(output_path, data).map_err(|e| e.to_string())?;
        Ok::<(), String>(())
    })();

    let _ = remove_file(&raw_path);
    result
}

pub(crate) fn doctor_patch_file(
    doctor_dir: &Path,
    drive: &str,
    fat_path: &str,
    replacement_file: &Path,
) -> Result<(), String> {
    if !replacement_file.exists() {
        return Err(format!(
            "replacement file '{}' does not exist",
            replacement_file.display()
        ));
    }

    let replacement = std::fs::read(replacement_file).map_err(|e| e.to_string())?;
    let mount = read_doctor_mount(doctor_dir, drive)?;
    let mount_dir = doctor_dir.join(&mount.dir);
    let raw_path = temp_raw_path("patch", drive);
    restore_sockdrive_raw_from_dir(&mount_dir, &raw_path)?;

    let result = (|| {
        let mut raw = File::options()
            .read(true)
            .write(true)
            .open(&raw_path)
            .map_err(|e| e.to_string())?;
        let fat = read_fat32_info(&mut raw)?;
        let entry = resolve_fat32_path(&mut raw, &fat, fat_path)?
            .ok_or_else(|| format!("FAT32 file '{}' not found", fat_path))?;
        if entry.is_dir() {
            return Err(format!("FAT32 path '{}' is a directory", fat_path));
        }
        if entry.size as usize != replacement.len() {
            return Err(format!(
                "replacement size mismatch: {} != {}",
                replacement.len(),
                entry.size
            ));
        }

        let extents = file_data_extents(&mut raw, &fat, &entry)?;
        write_replacement_to_raw(&mut raw, &extents, &replacement)?;
        patch_sockdrive_ranges_from_raw(&mount_dir, &mut raw, &extents)?;
        println!(
            "Patched '{}' in drive {} using {}",
            fat_path,
            drive,
            replacement_file.display()
        );
        Ok::<(), String>(())
    })();

    let _ = remove_file(&raw_path);
    result
}

fn temp_raw_path(prefix: &str, drive: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "sockdrive-doctor-{}-{}-{}-{}.raw",
        prefix,
        sanitize_filename(drive),
        std::process::id(),
        nanos
    ))
}

pub(crate) fn restore_sockdrive_raw_from_dir(
    mount_dir: &Path,
    raw_path: &Path,
) -> Result<(), String> {
    let (_, meta, _) = read_sockdrive_meta_file(&mount_dir.join("sockdrive.metaj"))?;
    let raw_size = meta.size_kb * 1024;
    let mut raw = File::create(raw_path).map_err(|e| e.to_string())?;
    raw.set_len(raw_size).map_err(|e| e.to_string())?;

    let dropped: HashSet<u32> = meta.dropped_ranges.iter().copied().collect();
    let small: HashSet<u32> = meta.small_ranges.iter().copied().collect();
    let preload = if meta.small_ranges.is_empty() {
        Vec::new()
    } else {
        decode_file_len(
            &mount_dir.join("preload.raw"),
            meta.small_ranges.len() * meta.ahead_read as usize,
        )?
        .0
    };

    for range in 0..meta.range_count {
        let range_u32 = range as u32;
        let offset = range * meta.ahead_read;
        if offset >= raw_size || dropped.contains(&range_u32) {
            continue;
        }

        let range_data =
            if let Some(small_index) = meta.small_ranges.iter().position(|r| *r == range_u32) {
                let start = small_index * meta.ahead_read as usize;
                preload[start..start + meta.ahead_read as usize].to_vec()
            } else if small.contains(&range_u32) {
                return Err(format!("small range {} is duplicated", range));
            } else {
                decode_file_len(
                    &mount_dir.join(format!("{}.raw", range)),
                    meta.ahead_read as usize,
                )?
                .0
            };

        let write_len = min(meta.ahead_read, raw_size - offset) as usize;
        raw.seek(std::io::SeekFrom::Start(offset))
            .map_err(|e| e.to_string())?;
        raw.write_all(&range_data[..write_len])
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

fn patch_sockdrive_ranges_from_raw(
    mount_dir: &Path,
    raw: &mut File,
    extents: &[(u64, u64)],
) -> Result<(), String> {
    let meta_path = mount_dir.join("sockdrive.metaj");
    let (mut meta_value, mut meta, meta_encoding) = read_sockdrive_meta_file(&meta_path)?;
    let raw_size = meta.size_kb * 1024;
    let mut affected = HashSet::new();
    for (offset, len) in extents {
        if *len == 0 {
            continue;
        }
        let first = offset / meta.ahead_read;
        let last = (offset + len - 1) / meta.ahead_read;
        for range in first..=last {
            affected.insert(range);
        }
    }

    let mut affected: Vec<u64> = affected.into_iter().collect();
    affected.sort_unstable();

    let mut preload = None::<(Vec<u8>, StoredEncoding)>;
    let mut dropped_changed = false;
    for range in affected {
        if range >= meta.range_count {
            return Err(format!("affected range {} is outside range_count", range));
        }

        let range_data = read_raw_range(raw, range, &meta, raw_size)?;
        let range_u32 = range as u32;
        if let Some(small_index) = meta.small_ranges.iter().position(|r| *r == range_u32) {
            if preload.is_none() {
                preload = Some(decode_file_len(
                    &mount_dir.join("preload.raw"),
                    meta.small_ranges.len() * meta.ahead_read as usize,
                )?);
            }
            let (preload_data, _) = preload.as_mut().unwrap();
            let start = small_index * meta.ahead_read as usize;
            preload_data[start..start + meta.ahead_read as usize].copy_from_slice(&range_data);
        } else {
            let range_path = mount_dir.join(format!("{}.raw", range));
            let encoding = if range_path.exists() {
                decode_file_len(&range_path, meta.ahead_read as usize)?.1
            } else if meta.dropped_ranges.contains(&range_u32) {
                meta.dropped_ranges.retain(|r| *r != range_u32);
                dropped_changed = true;
                StoredEncoding::Plain
            } else {
                return Err(format!(
                    "range file '{}' does not exist",
                    range_path.display()
                ));
            };
            encode_file(&range_path, &range_data, encoding)?;
        }
    }

    if let Some((preload_data, encoding)) = preload {
        encode_file(&mount_dir.join("preload.raw"), &preload_data, encoding)?;
    }

    if dropped_changed {
        meta_value.as_object_mut().unwrap().insert(
            String::from("dropped_ranges"),
            serde_json::json!(meta.dropped_ranges),
        );
        let text = serde_json::to_string(&meta_value).map_err(|e| e.to_string())?;
        encode_file(&meta_path, text.as_bytes(), meta_encoding)?;
    }

    Ok(())
}

fn read_raw_range(
    raw: &mut File,
    range: u64,
    meta: &SockdriveMeta,
    raw_size: u64,
) -> Result<Vec<u8>, String> {
    let offset = range * meta.ahead_read;
    let mut data = vec![0u8; meta.ahead_read as usize];
    if offset < raw_size {
        let read_len = min(meta.ahead_read, raw_size - offset) as usize;
        raw.seek(std::io::SeekFrom::Start(offset))
            .map_err(|e| e.to_string())?;
        raw.read_exact(&mut data[..read_len])
            .map_err(|e| e.to_string())?;
    }
    Ok(data)
}

fn read_sockdrive_meta_file(
    path: &Path,
) -> Result<(serde_json::Value, SockdriveMeta, StoredEncoding), String> {
    let (meta_bytes, encoding) = decode_json_file(path)?;
    let meta_value: serde_json::Value = serde_json::from_slice(&meta_bytes)
        .map_err(|e| format!("invalid sockdrive.metaj JSON: {}", e))?;
    let meta = parse_sockdrive_meta(&meta_value)?;
    validate_sockdrive_meta(&meta)?;
    Ok((meta_value, meta, encoding))
}
