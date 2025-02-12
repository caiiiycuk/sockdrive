use std::fs::{copy, create_dir, metadata, read_dir, remove_file, rename, write, File};
use std::io::{Read, Seek, Write};
use std::process::Command;

const AHEAD_READ_KB: u32 = 256;
const SECTOR_SIZE: u32 = 512;
const AHEAD_READ_SECTORS: u32 = AHEAD_READ_KB * 1024 / SECTOR_SIZE;
const FAT16_256MB: &str = include_str!("../drives/fat16-256mb.json");
const FAT32_2GB: &str = include_str!("../drives/fat32-2gb.json");

fn main() {
    println!();

    let args: Vec<String> = std::env::args().collect();
    if args.len() != 5 || args[1] != "mkd" {
        eprintln!(
            "
sockdrive cli

Usage:
    sockdrive mkd <raw_image> <preload_sectors> <output_dir> [-b]

    raw_image: path to the raw image file
    preload_sectors: comma separated list of sectors to preload on startup
    output_dir: path to the output directory
    -b: enable brotli compression (brotli cmd should be in PATH)
Note:
    you can use '_' as a preload sector to preload all sectors

Example:
    sockdrive mkd win95v1.raw ./output
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

    if std::path::Path::new(&output_dir).exists() {
        eprintln!("Error: Output directory '{}' exists", output_dir);
        std::process::exit(1);
    };

    create_dir(&output_dir).unwrap();

    if preload == "_" {
        copy(&input_file, format!("{}/_.raw", output_dir)).unwrap();
    } else {
        let mut preload_sectors = preload
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();

        mkd(&input_file, &mut preload_sectors, &output_dir);
        config.as_object_mut().unwrap().insert(
            String::from("preload_sectors"),
            serde_json::json!(preload_sectors),
        );
    };

    write(
        format!("{}/sockdrive.json", output_dir),
        serde_json::to_string(&config).unwrap(),
    )
    .unwrap();

    if args.contains(&"-b".to_string()) {
        brotli_all(&output_dir);
    }
}

fn mkd(raw_image: &str, preload_sectors: &mut Vec<u32>, output_dir: &str) {
    let mut raw = File::open(raw_image).unwrap();
    let mut buffer = vec![0u8; SECTOR_SIZE as usize];

    let sectors = raw.metadata().unwrap().len() as u32 / SECTOR_SIZE;

    let mut preload_sectors = preload_sectors.to_vec();
    if preload_sectors.is_empty() {
        for sector in 0..sectors {
            preload_sectors.push(sector);
        }
    }

    // making preload file
    let mut preload_file = File::create(format!("{}/_.raw", output_dir)).unwrap();
    for sector in preload_sectors.to_owned() {
        raw.seek(std::io::SeekFrom::Start(
            (sector as u64) * SECTOR_SIZE as u64,
        ))
        .unwrap();
        raw.read_exact(&mut buffer).unwrap();
        preload_file.write_all(&buffer).unwrap();
    }

    // making ahead files
    raw.seek(std::io::SeekFrom::Start(0)).unwrap();
    for i in 0..sectors / AHEAD_READ_SECTORS as u32 {
        let mut ahead_file = File::create(format!("{}/{}.raw", output_dir, i)).unwrap();
        let mut size = 0;
        for sector in 0..AHEAD_READ_SECTORS as u32 {
            let sector = i * AHEAD_READ_SECTORS + sector as u32;
            if !preload_sectors.contains(&sector) {
                raw.read_exact(&mut buffer).unwrap();
                if buffer.iter().find(|&x| *x != 0).is_some() {
                    size += SECTOR_SIZE;
                }
                ahead_file.write_all(&buffer).unwrap();
            }
        }
        if size == 0 {
            ahead_file.flush().unwrap();
            remove_file(format!("{}/{}.raw", output_dir, i)).unwrap();
        }
    }
}

fn brotli_all(output_dir: &str) {
    let files: Vec<_> = read_dir(output_dir).unwrap().flatten().collect();
    let num_cpus = num_cpus::get();
    let chunks = files.chunks(files.len().div_ceil(num_cpus));

    let total = chunks.len();
    println!("Compressing {} files on {} CPUs", files.len(), num_cpus);
    chunks.enumerate().for_each(|(i, chunk)| {
        let handles: Vec<_> = chunk
            .iter()
            .map(|file| {
                let path = file.path();
                std::thread::spawn(move || {
                    let status = Command::new("brotli")
                        .arg("-Z")
                        .arg(&path)
                        .status()
                        .unwrap();

                    if !status.success() {
                        eprintln!("Failed to compress {:?}", path);
                        std::process::exit(1);
                    }

                    let br_path = path.with_extension("raw.br");
                    let orig_size = metadata(&path).unwrap().len();
                    let br_size = metadata(&br_path).unwrap().len();

                    if br_size < orig_size {
                        rename(br_path, path).unwrap();
                    } else {
                        remove_file(&br_path).unwrap();
                        let status = Command::new("brotli")
                            .arg("-0")
                            .arg(&path)
                            .status()
                            .unwrap();

                        if !status.success() {
                            eprintln!("Failed to compress {:?}", path);
                            std::process::exit(1);
                        }

                        rename(br_path, path).unwrap();
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
