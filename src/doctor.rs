use crate::fat32::{
    file_data_extents, list_fat32_files_recursive, list_fat32_path, read_fat32_info,
    read_file_extents, resolve_fat32_path,
};
use crate::{
    apply_sockdrive_changes, convert_raw_to_qcow2, copy_dir_all, decode_file_len, decode_json_file,
    download_sockdrive_file, encode_file, find_sockdrive_mounts, normalize_sockdrive_url,
    optional_u32_array, parse_sockdrive_changes, parse_sockdrive_meta, sanitize_filename,
    validate_sockdrive_meta, SockdriveMeta, SockdriveMount, StoredEncoding,
};
use std::cmp::min;
use std::collections::{HashMap, HashSet};
use std::fs::{create_dir_all, read_dir, remove_file, File};
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
        && args[2] != "br"
        && args[2] != "gz"
        && args[2] != "verify"
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

    if args.len() >= 3 && args[2] == "verify" {
        let fat32_mode = args.iter().skip(3).any(|arg| arg == "--fat32");
        let verify_args: Vec<&str> = args
            .iter()
            .skip(3)
            .filter_map(|arg| {
                if arg == "--fat32" {
                    None
                } else {
                    Some(arg.as_str())
                }
            })
            .collect();
        if verify_args.len() != 3 {
            doctor_usage();
            std::process::exit(1);
        }

        if fat32_mode {
            let report = doctor_verify_fat32(
                Path::new(verify_args[0]),
                verify_args[1],
                Path::new(verify_args[2]),
            )
            .unwrap_or_else(|e| {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            });
            print_fat32_verify_report(&report);
        } else {
            let mismatches = doctor_verify_folder(
                Path::new(verify_args[0]),
                verify_args[1],
                Path::new(verify_args[2]),
            )
            .unwrap_or_else(|e| {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            });
            for mismatch in mismatches {
                println!("{}", mismatch);
            }
        }
        return;
    }

    if args.len() >= 3 && (args[2] == "br" || args[2] == "gz") {
        let doctor_dir = match args.len() {
            3 => std::env::current_dir().unwrap_or_else(|e| {
                eprintln!("Error: failed to get current directory: {}", e);
                std::process::exit(1);
            }),
            4 => PathBuf::from(&args[3]),
            _ => {
                doctor_usage();
                std::process::exit(1);
            }
        };
        let encoding = if args[2] == "br" {
            StoredEncoding::Brotli
        } else {
            StoredEncoding::Gzip
        };
        doctor_compress(&doctor_dir, encoding).unwrap_or_else(|e| {
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
    sockdrive doctor verify <doctor_dir> <drive> <folder>
    sockdrive doctor verify --fat32 <doctor_dir> <drive> <folder>
    sockdrive doctor br [doctor_dir]
    sockdrive doctor gz [doctor_dir]

    jsdos_bundle: path to a jsdos bundle containing sockdrive mounts
    doctor_dir: directory created by `sockdrive doctor <jsdos_bundle>`
    drive: DOSBox imgmount drive id, for example 2 or 3
    fat_path: path inside the FAT32 filesystem
    extract writes to the current directory using the FAT32 file name
    restore writes qcow2 images into doctor_dir
    verify compares optimized drive folders, without recursion
    verify --fat32 compares FAT32 files against recursive folder files by name, ignore case
    br/gz write recompressed publishing copies to <doctor_dir>.br or <doctor_dir>.gz
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

#[derive(Clone)]
enum CompressTaskKind {
    Json,
    RawLen(usize),
}

#[derive(Clone)]
struct CompressTask {
    source_path: PathBuf,
    output_path: PathBuf,
    kind: CompressTaskKind,
}

fn doctor_compress(doctor_dir: &Path, encoding: StoredEncoding) -> Result<(), String> {
    if !doctor_dir.is_dir() {
        return Err(format!(
            "doctor directory '{}' does not exist",
            doctor_dir.display()
        ));
    }

    let output_dir = compressed_doctor_output_dir(doctor_dir, encoding)?;
    if output_dir.exists() {
        return Err(format!(
            "compressed doctor directory '{}' exists",
            output_dir.display()
        ));
    }

    let tasks = collect_compress_tasks(doctor_dir, &output_dir)?;
    if tasks.is_empty() {
        println!("Nothing to compress in {}", doctor_dir.display());
        return Ok(());
    }

    if let Err(e) = create_compressed_doctor_dirs(doctor_dir, &output_dir) {
        let _ = std::fs::remove_dir_all(&output_dir);
        return Err(e);
    }
    let label = encoding_label(encoding);
    let workers = min(
        tasks.len(),
        std::thread::available_parallelism()
            .map(|count| count.get())
            .unwrap_or(4),
    );
    let queue = Arc::new(Mutex::new(std::collections::VecDeque::from(tasks)));
    let error = Arc::new(Mutex::new(None::<String>));
    let completed = Arc::new(AtomicUsize::new(0));
    let total = queue.lock().unwrap().len();
    let mut handles = Vec::new();

    eprintln!(
        "Compressing {} doctor file(s) to {} with {} workers",
        total, label, workers
    );

    for _ in 0..workers {
        let queue = Arc::clone(&queue);
        let error = Arc::clone(&error);
        let completed = Arc::clone(&completed);

        handles.push(thread::spawn(move || loop {
            if error.lock().unwrap().is_some() {
                break;
            }

            let Some(task) = queue.lock().unwrap().pop_front() else {
                break;
            };

            if let Err(e) = compress_doctor_file(&task, encoding) {
                *error.lock().unwrap() = Some(e);
                break;
            }

            let done = completed.fetch_add(1, Ordering::SeqCst) + 1;
            if done == total || done.is_multiple_of(100) {
                eprintln!("Compressed {}/{} doctor files", done, total);
            }
        }));
    }

    for handle in handles {
        handle
            .join()
            .map_err(|_| "doctor compression worker panicked".to_string())?;
    }

    if let Some(error) = error.lock().unwrap().take() {
        let _ = std::fs::remove_dir_all(&output_dir);
        return Err(error);
    }

    println!(
        "Done, compressed {} doctor file(s) to {} in {}",
        total,
        label,
        output_dir.display()
    );
    Ok(())
}

fn compressed_doctor_output_dir(
    doctor_dir: &Path,
    encoding: StoredEncoding,
) -> Result<PathBuf, String> {
    let suffix = match encoding {
        StoredEncoding::Brotli => "br",
        StoredEncoding::Gzip => "gz",
        StoredEncoding::Plain => return Err("plain compression is not supported".to_string()),
    };
    let source_dir = if doctor_dir == Path::new(".") {
        std::env::current_dir().map_err(|e| e.to_string())?
    } else {
        doctor_dir.to_path_buf()
    };
    let name = source_dir
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            format!(
                "failed to build compressed directory name for '{}'",
                doctor_dir.display()
            )
        })?;
    Ok(source_dir.with_file_name(format!("{}.{}", name, suffix)))
}

fn create_compressed_doctor_dirs(doctor_dir: &Path, output_dir: &Path) -> Result<(), String> {
    let mounts = read_doctor_manifest(doctor_dir)?;
    create_dir_all(output_dir).map_err(|e| e.to_string())?;
    std::fs::copy(
        doctor_dir.join("doctor.json"),
        output_dir.join("doctor.json"),
    )
    .map_err(|e| e.to_string())?;
    for mount in mounts {
        create_dir_all(output_dir.join(mount.dir)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn collect_compress_tasks(
    doctor_dir: &Path,
    output_dir: &Path,
) -> Result<Vec<CompressTask>, String> {
    let mounts = read_doctor_manifest(doctor_dir)?;
    if mounts.is_empty() {
        return Err("doctor.json contains no mounts".to_string());
    }

    let mut tasks = Vec::new();
    for mount in mounts {
        let mount_dir = doctor_dir.join(&mount.dir);
        if !mount_dir.is_dir() {
            return Err(format!(
                "mount directory '{}' does not exist",
                mount_dir.display()
            ));
        }

        let meta_path = mount_dir.join("sockdrive.metaj");
        let (_, meta, _) = read_sockdrive_meta_file(&meta_path)?;
        tasks.push(CompressTask {
            source_path: meta_path,
            output_path: output_dir.join(&mount.dir).join("sockdrive.metaj"),
            kind: CompressTaskKind::Json,
        });

        if !meta.small_ranges.is_empty() {
            let preload_path = mount_dir.join("preload.raw");
            if !preload_path.exists() {
                return Err(format!(
                    "preload file '{}' does not exist",
                    preload_path.display()
                ));
            }
            tasks.push(CompressTask {
                source_path: preload_path,
                output_path: output_dir.join(&mount.dir).join("preload.raw"),
                kind: CompressTaskKind::RawLen(meta.small_ranges.len() * meta.ahead_read as usize),
            });
        }

        let dropped: HashSet<u32> = meta.dropped_ranges.iter().copied().collect();
        let small: HashSet<u32> = meta.small_ranges.iter().copied().collect();
        let raw_size = meta.size_kb * 1024;
        for range in 0..meta.range_count {
            let range_u32 = range as u32;
            if range * meta.ahead_read >= raw_size
                || dropped.contains(&range_u32)
                || small.contains(&range_u32)
            {
                continue;
            }

            let range_path = mount_dir.join(format!("{}.raw", range));
            if !range_path.exists() {
                return Err(format!(
                    "range file '{}' does not exist",
                    range_path.display()
                ));
            }
            tasks.push(CompressTask {
                source_path: range_path,
                output_path: output_dir.join(&mount.dir).join(format!("{}.raw", range)),
                kind: CompressTaskKind::RawLen(meta.ahead_read as usize),
            });
        }
    }

    Ok(tasks)
}

fn compress_doctor_file(task: &CompressTask, encoding: StoredEncoding) -> Result<(), String> {
    let data = match task.kind {
        CompressTaskKind::Json => decode_json_file(&task.source_path)?.0,
        CompressTaskKind::RawLen(expected_len) => {
            decode_file_len(&task.source_path, expected_len)?.0
        }
    };
    encode_file_mkd_style(&task.output_path, &data, encoding)
}

fn encode_file_mkd_style(
    output_path: &Path,
    data: &[u8],
    encoding: StoredEncoding,
) -> Result<(), String> {
    let (command_name, best_arg, fast_arg, suffix) = match encoding {
        StoredEncoding::Brotli => ("brotli", "-Z", "-0", "br"),
        StoredEncoding::Gzip => ("gzip", "-9", "-1", "gz"),
        StoredEncoding::Plain => return Err("plain compression is not supported".to_string()),
    };
    let (compressed, compressed_len) =
        compress_to_temp(output_path, data, command_name, best_arg, suffix)?;
    if compressed_len < data.len() as u64 {
        std::fs::rename(&compressed, output_path).map_err(|e| e.to_string())?;
        return Ok(());
    }

    let _ = remove_file(&compressed);
    let (compressed, _) = compress_to_temp(output_path, data, command_name, fast_arg, suffix)?;
    std::fs::rename(&compressed, output_path).map_err(|e| e.to_string())
}

fn compress_to_temp(
    output_path: &Path,
    data: &[u8],
    command_name: &str,
    compression_arg: &str,
    suffix: &str,
) -> Result<(PathBuf, u64), String> {
    let tmp = output_path.with_extension(format!(
        "compress-tmp-{}-{}",
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
            .unwrap_or("compress-tmp"),
        suffix
    ));

    std::fs::write(&tmp, data).map_err(|e| e.to_string())?;
    let status = Command::new(command_name)
        .arg(compression_arg)
        .arg("-k")
        .arg(&tmp)
        .status()
        .map_err(|e| format!("failed to run {}: {}", command_name, e))?;
    let _ = remove_file(&tmp);
    if !status.success() {
        let _ = remove_file(&compressed_tmp);
        return Err(format!("{} failed", command_name));
    }

    let compressed_len = std::fs::metadata(&compressed_tmp)
        .map_err(|e| e.to_string())?
        .len();
    Ok((compressed_tmp, compressed_len))
}

fn encoding_label(encoding: StoredEncoding) -> &'static str {
    match encoding {
        StoredEncoding::Plain => "plain",
        StoredEncoding::Gzip => "gzip",
        StoredEncoding::Brotli => "brotli",
    }
}

struct LocalVerifyEntry {
    name: String,
    path: PathBuf,
    is_dir: bool,
}

pub(crate) struct Fat32VerifyReport {
    pub(crate) matched: usize,
    pub(crate) not_found: usize,
    pub(crate) not_matched: Vec<String>,
    pub(crate) ambiguous_match: Vec<String>,
}

fn print_fat32_verify_report(report: &Fat32VerifyReport) {
    println!("MATCHED - {} files", report.matched);
    println!("NOT FOUND - {} files", report.not_found);
    println!("AMBIGUOUS MATCH - {} files", report.ambiguous_match.len());
    for path in &report.ambiguous_match {
        println!("- {}", path);
    }
    println!("NOT MATCHED:");
    for path in &report.not_matched {
        println!("- {}", path);
    }
}

pub(crate) fn doctor_verify_folder(
    doctor_dir: &Path,
    drive: &str,
    folder: &Path,
) -> Result<Vec<String>, String> {
    let mount = read_doctor_mount(doctor_dir, drive)?;
    let source_dir = doctor_dir.join(&mount.dir);
    if !source_dir.is_dir() {
        return Err(format!(
            "mount directory '{}' does not exist",
            source_dir.display()
        ));
    }
    let target_dir = resolve_verify_target_dir(folder, drive)?;
    compare_verify_dirs(&source_dir, &target_dir, &mount.dir)
}

pub(crate) fn doctor_verify_fat32(
    doctor_dir: &Path,
    drive: &str,
    folder: &Path,
) -> Result<Fat32VerifyReport, String> {
    if !folder.is_dir() {
        return Err(format!("folder '{}' does not exist", folder.display()));
    }

    let local_files = read_recursive_verify_files(folder)?;
    let mount = read_doctor_mount(doctor_dir, drive)?;
    let mount_dir = doctor_dir.join(&mount.dir);
    let raw_path = temp_raw_path("verify-fat32", drive);
    restore_sockdrive_raw_from_dir(&mount_dir, &raw_path)?;

    let result = (|| {
        let mut raw = File::open(&raw_path).map_err(|e| e.to_string())?;
        let fat = read_fat32_info(&mut raw)?;
        let mut matched = 0usize;
        let mut not_found = 0usize;
        let mut not_matched = Vec::new();
        let mut ambiguous_match = Vec::new();

        for fat_file in list_fat32_files_recursive(&mut raw, &fat)? {
            let Some(local_matches) = local_files.get(&verify_name_key(&fat_file.name)) else {
                not_found += 1;
                continue;
            };
            if local_matches.len() > 1 {
                ambiguous_match.push(fat_file.path);
                continue;
            }
            let local_file = &local_matches[0];
            let local_data = std::fs::read(&local_file.path).map_err(|e| e.to_string())?;
            if local_data.len() != fat_file.entry.size as usize {
                not_matched.push(fat_file.path);
                continue;
            }

            let extents = file_data_extents(&mut raw, &fat, &fat_file.entry)?;
            let fat_data = read_file_extents(&mut raw, &extents)?;
            if fat_data == local_data {
                matched += 1;
            } else {
                not_matched.push(fat_file.path);
            }
        }

        not_matched.sort();
        ambiguous_match.sort();
        Ok::<Fat32VerifyReport, String>(Fat32VerifyReport {
            matched,
            not_found,
            not_matched,
            ambiguous_match,
        })
    })();

    let _ = remove_file(&raw_path);
    result
}

fn resolve_verify_target_dir(folder: &Path, drive: &str) -> Result<PathBuf, String> {
    if !folder.is_dir() {
        return Err(format!("folder '{}' does not exist", folder.display()));
    }
    if folder.join("doctor.json").exists() {
        let mount = read_doctor_mount(folder, drive)?;
        let mount_dir = folder.join(&mount.dir);
        if !mount_dir.is_dir() {
            return Err(format!(
                "mount directory '{}' does not exist",
                mount_dir.display()
            ));
        }
        return Ok(mount_dir);
    }

    Ok(folder.to_path_buf())
}

fn compare_verify_dirs(
    source_dir: &Path,
    target_dir: &Path,
    output_prefix: &str,
) -> Result<Vec<String>, String> {
    let source_entries = read_local_verify_entries(source_dir)?;
    let mut target_entries = read_local_verify_entries(target_dir)?;
    let mut mismatches = Vec::new();

    for (_, source_entry) in source_entries {
        let Some(target_entry) = target_entries.remove(&verify_name_key(&source_entry.name)) else {
            mismatches.push(format!("{}/{}", output_prefix, source_entry.name));
            continue;
        };

        if source_entry.is_dir || target_entry.is_dir {
            if source_entry.is_dir != target_entry.is_dir {
                mismatches.push(format!("{}/{}", output_prefix, source_entry.name));
            }
            continue;
        }

        let source_data = std::fs::read(&source_entry.path).map_err(|e| e.to_string())?;
        let target_data = std::fs::read(&target_entry.path).map_err(|e| e.to_string())?;
        if source_data != target_data {
            mismatches.push(format!("{}/{}", output_prefix, source_entry.name));
        }
    }

    for (_, entry) in target_entries {
        mismatches.push(format!("{}/{}", output_prefix, entry.name));
    }

    mismatches.sort();
    Ok(mismatches)
}

fn read_local_verify_entries(folder: &Path) -> Result<HashMap<String, LocalVerifyEntry>, String> {
    let mut entries = HashMap::new();
    for entry in read_dir(folder).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let file_type = entry.file_type().map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().to_string();
        let key = verify_name_key(&name);
        if entries
            .insert(
                key.clone(),
                LocalVerifyEntry {
                    name,
                    path: entry.path(),
                    is_dir: file_type.is_dir(),
                },
            )
            .is_some()
        {
            return Err(format!(
                "folder '{}' contains duplicate case-insensitive entry '{}'",
                folder.display(),
                key
            ));
        }
    }
    Ok(entries)
}

fn read_recursive_verify_files(
    folder: &Path,
) -> Result<HashMap<String, Vec<LocalVerifyEntry>>, String> {
    let mut files = HashMap::new();
    read_recursive_verify_files_at(folder, &mut files)?;
    Ok(files)
}

fn read_recursive_verify_files_at(
    folder: &Path,
    files: &mut HashMap<String, Vec<LocalVerifyEntry>>,
) -> Result<(), String> {
    for entry in read_dir(folder).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let file_type = entry.file_type().map_err(|e| e.to_string())?;
        if file_type.is_dir() {
            read_recursive_verify_files_at(&entry.path(), files)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }

        let name = entry.file_name().to_string_lossy().to_string();
        let key = verify_name_key(&name);
        files.entry(key).or_default().push(LocalVerifyEntry {
            name,
            path: entry.path(),
            is_dir: false,
        });
    }

    Ok(())
}

fn verify_name_key(name: &str) -> String {
    name.to_lowercase()
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
) -> Result<Vec<String>, String> {
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
        let changed_extents = write_changed_replacement_to_raw(&mut raw, &extents, &replacement)?;
        let updated_files =
            patch_sockdrive_ranges_from_raw(&mount_dir, &mount.dir, &mut raw, &changed_extents)?;
        for file in &updated_files {
            println!("{}", file);
        }
        Ok::<Vec<String>, String>(updated_files)
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
    mount_dir_name: &str,
    raw: &mut File,
    extents: &[(u64, u64)],
) -> Result<Vec<String>, String> {
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
    let mut updated_files = Vec::new();
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
            let preload_file = format!("{}/preload.raw", mount_dir_name);
            if !updated_files.iter().any(|file| file == &preload_file) {
                updated_files.push(preload_file);
            }
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
            updated_files.push(format!("{}/{}.raw", mount_dir_name, range));
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

    Ok(updated_files)
}

fn write_changed_replacement_to_raw(
    raw: &mut File,
    extents: &[(u64, u64)],
    replacement: &[u8],
) -> Result<Vec<(u64, u64)>, String> {
    let mut source_offset = 0usize;
    let mut changed_extents = Vec::new();
    for (raw_offset, len) in extents {
        let len = *len as usize;
        let replacement_slice = &replacement[source_offset..source_offset + len];
        let mut original = vec![0u8; len];
        raw.seek(std::io::SeekFrom::Start(*raw_offset))
            .map_err(|e| e.to_string())?;
        raw.read_exact(&mut original).map_err(|e| e.to_string())?;

        for (start, end) in changed_runs(&original, replacement_slice) {
            raw.seek(std::io::SeekFrom::Start(*raw_offset + start as u64))
                .map_err(|e| e.to_string())?;
            raw.write_all(&replacement_slice[start..end])
                .map_err(|e| e.to_string())?;
            changed_extents.push((*raw_offset + start as u64, (end - start) as u64));
        }

        source_offset += len;
    }

    Ok(changed_extents)
}

fn changed_runs(original: &[u8], replacement: &[u8]) -> Vec<(usize, usize)> {
    let mut runs = Vec::new();
    let mut offset = 0usize;
    while offset < replacement.len() {
        if original[offset] == replacement[offset] {
            offset += 1;
            continue;
        }

        let start = offset;
        offset += 1;
        while offset < replacement.len() && original[offset] != replacement[offset] {
            offset += 1;
        }
        runs.push((start, offset));
    }
    runs
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
