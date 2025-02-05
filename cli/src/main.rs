use std::fs::File;
use std::io::{Read, Seek, Write};
use std::process::Command;

const AHEAD_READ_KB: u32 = 256;
const SECTOR_SIZE: u32 = 512;
const AHEAD_READ_SECTORS: u32 = AHEAD_READ_KB * 1024 / SECTOR_SIZE;

fn main() {
    println!();

    let args: Vec<String> = std::env::args().collect();
    if args.len() != 5 || args[1] != "mkd" {
        eprintln!(
            "
sockdrive cli

Usage:
    sockdrive mkd <raw_image> <preload_sectors> <output_dir>

    raw_image: path to the raw image file
    preload_sectors: comma separated list of sectors to preload on startup
    output_dir: path to the output directory

Example:
    sockdrive mkd win95v1.raw ./output
        "
        );
        std::process::exit(1);
    }

    if !std::path::Path::new(&args[2]).exists() {
        eprintln!("Error: Input file '{}' does not exist", args[2]);
        std::process::exit(1);
    }

    let preload_sectors: Vec<u32> = args[3]
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();

    if std::path::Path::new(&args[4]).exists() {
        eprintln!("Error: Output directory '{}' exists", args[4]);
        std::process::exit(1);
    }

    mkd(&args[2], &preload_sectors, &args[4]);
    brotli_all(&args[4]);
}

fn mkd(raw_image: &str, preload_sectors: &[u32], output_dir: &str) {
    let mut raw = File::open(raw_image).unwrap();
    let mut buffer = vec![0u8; SECTOR_SIZE as usize];
    std::fs::create_dir(output_dir).unwrap();

    let sectors = raw.metadata().unwrap().len() as u32 / SECTOR_SIZE;

    // making preload file
    let mut preload_file = File::create(format!("{}/_.raw", output_dir)).unwrap();
    for &sector in preload_sectors {
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
            std::fs::remove_file(format!("{}/{}.raw", output_dir, i)).unwrap();
        }
    }
}

fn brotli_all(output_dir: &str) {
    let files = std::fs::read_dir(output_dir).unwrap();
    for file in files {
        let path = file.unwrap().path();

        let status = Command::new("brotli")
            .arg("-Z")
            .arg(&path)
            .status()
            .unwrap();

        if !status.success() {
            eprintln!("Failed to compress {:?}", path);
            std::process::exit(1);
        }
    }
}
