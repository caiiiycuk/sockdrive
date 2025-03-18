use std::fs::{create_dir, metadata, read_dir, remove_file, rename, write, File};
use std::io::{Read, Seek, Write};
use std::process::Command;

const AHEAD_READ_SIZE: u64 = 256 * 1024;
const FAT16_256MB: &str = include_str!("../drives/fat16-256mb.json");
const FAT32_2GB: &str = include_str!("../drives/fat32-2gb.json");

fn main() {
    task(std::env::args().collect());
}

fn task(args: Vec<String>) {
    if args.len() < 5 || args[1] != "mkd" {
        eprintln!(
            "
sockdrive cli

Usage:
    sockdrive mkd <raw_image> <preload_ranges> <output_dir> [-b]

    raw_image: path to the raw image file
    preload_ranges: comma separated list of ranges to preload on startup (range is index, range size is AHEAD_READ_SIZE(256 * 1024))
    output_dir: path to the output directory
    -b: enable brotli compression (brotli cmd should be in PATH)
Note:
    you can use '_' as a preload sector to preload all ranges

Example:
    sockdrive mkd win95v1.raw ./output [-b]
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

    let input_size = metadata(input_file).unwrap().len();
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

    let dropped = mkahead(input_file, range_count as u32, output_dir);
    config
        .as_object_mut()
        .unwrap()
        .insert(String::from("dropped_ranges"), serde_json::json!(dropped));

    let preload_ranges: Vec<u32> = if preload == "_" {
        (0..range_count as u32).collect()
    } else {
        preload
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect()
    };

    for range in preload_ranges.iter() {
        assert!(
            *range < range_count as u32,
            "range {} is greater then range count {}",
            range,
            range_count
        );
    }

    if preload == "_" {
        config
            .as_object_mut()
            .unwrap()
            .insert(String::from("preload_ranges"), serde_json::json!("_"));
    } else {
        let preload_ranges: Vec<u32> = preload_ranges
            .iter()
            .filter(|range| !dropped.contains(range))
            .copied()
            .collect();

        config.as_object_mut().unwrap().insert(
            String::from("preload_ranges"),
            serde_json::json!(preload_ranges),
        );
    }

    write(
        format!("{}/sockdrive.metaj", output_dir),
        serde_json::to_string(&config).unwrap(),
    )
    .unwrap();

    if args.contains(&"-b".to_string()) {
        brotli_all(output_dir);
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
                        .arg("-Zk")
                        .arg(&path)
                        .status()
                        .unwrap();

                    if !status.success() {
                        eprintln!("Failed to compress {:?}", path);
                        std::process::exit(1);
                    }

                    let br_path = format!("{}.br", &path.display());
                    let orig_size = metadata(&path).unwrap().len();
                    let br_size = match metadata(&br_path) {
                        Ok(meta) => meta.len(),
                        Err(_) => { 
                            println!("Failed to get metadata for {:?}", br_path);
                            std::process::exit(1);
                        },
                    };

                    if br_size < orig_size {
                        remove_file(&path).unwrap();
                        rename(br_path, path).unwrap();
                    } else {
                        remove_file(&br_path).unwrap();
                        let status = Command::new("brotli")
                            .arg("-0k")
                            .arg(&path)
                            .status()
                            .unwrap();

                        if !status.success() {
                            eprintln!("Failed to compress {:?}", path);
                            std::process::exit(1);
                        }

                        remove_file(&path).unwrap();
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::remove_dir_all;
    use std::path::Path;

    #[test]
    fn test_fat16_256mb_exists() {
        let path = Path::new("../test-assets/fat16-256mb.raw");
        assert!(path.exists(), "fat16-256mb.raw file should exist, please run `./test-assets/generate.sh` from root to generate it");
    }

    #[test]
    fn test_fat32_2gb_exists() {
        let path = Path::new("../test-assets/fat32-2gb.raw");
        assert!(path.exists(), "fat32-2gb.raw file should exist, please run `./test-assets/generate.sh` from root to generate it");
    }

    #[test]
    fn test_mkd_fat16_256mb() {
        test_mkd(
            "../test-assets/fat16-256mb.raw",
            &[0, 1, 2],
            "../test-assets/fat16-256mb",
        );
    }

    #[test]
    fn test_mkd_fat32_2gb() {
        test_mkd(
            "../test-assets/fat32-2gb.raw",
            &[0, 1, 2],
            "../test-assets/fat32-2gb",
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

        task(args);

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
}