use std::cmp::min;
use std::fs::{create_dir, create_dir_all, metadata, read_dir, remove_file, rename, write, File};
use std::io::{Read, Seek, Write};
use std::process::Command;

const AHEAD_READ_SIZE: u64 = 256 * 1024;
const FAT16_256MB: &str = include_str!("../drives/fat16-256mb.json");
const FAT32_2GB: &str = include_str!("../drives/fat32-2gb.json");
const SMALL_FILES_THRESHOLD: u64 = 1024 * 1024;
const DEFAULT_PRELOAD: &str = "0,16,1,52,50,68,51,145,280,152,291,227,234,226,207,279,7195,257,233,179,231,390,177,346,66,71,96,197,297,70,90,113,146,87,89,98,2,93,199,236,54,198,129,228,296,299,311,180,20,100,208,218,219,232,276,300,24,114,143,195,229,239,253,241,277,289,49,155,240,21,23,99,116,151,217,97,202,429,32,157,262,327,200,201,25,156,237,278,329,82,141,142,154,158,178,338,339,84,78,65,148,160,271,282,117,119,144,275,83,85,92,3,159,242,274,105,118,543,64,187,261,269,86,225,545,22,38,57,188,287,330,176,359,544,56,281,295,245,79,30,407,165,194,235,285,465,101,238,411,58,138,193,293,394,133,134,168,412,6,55,62,163,333,343,112,172,428,430,17,18,19,67,184,332,171,104,7,36,284,334,386,395,139,167,357,431,37,76,140,460,244,258,331,532,290,12,13,14,69,153,272,328,396,461,675,175,663,664,149,405,531,15,123,63,464,53,31,221,252,294,340,344,35,60,72,288,246,459,462,463,4,513,546,677,9,75,94,164,216,251,363,220,658,701,397,59,196,230,354,364,667,323,533,729";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 || (args[1] != "mkd" && args[1] != "sockify") {
        eprintln!(
            "
sockdrive cl

Use one of the following commands:
    sockdrive mkd   - make sockdrive from raw / qcow2 image
    sockdrive sockify - transform jsdos bundle with qcow2 images to use sockdrive
            "
        );
    } else if args[1] == "mkd" {
        mkd(args);
    } else if args[1] == "sockify" {
        sockify(args);
    }
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

Example:
    sockdrive sockify bundle.jsdos bundle- ./sockdrive https://my.site bundle-sockified.jsdos [-b]
    OR
    sockdirve sockify bindle-dir bundle- ./sockdrive https://my.site bindle-sockified.jsdos [-b]

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

    if imgmount_lines.len() == 0 {
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
        let outdrive = format!(
            "{}/{}{}",
            output_dir,
            drive_prefix,
            &path[..path.len() - ".qcow2".len()]
        );

        if std::path::Path::new(&outdrive).exists() {
            eprintln!("Error: drive '{}' already exists", outdrive);
            cleanup();
            std::process::exit(1);
        }

        if args.contains(&"-b".to_string()) {
            mkd(vec![
                "_".to_string(),
                "mkd".to_string(),
                indrive.clone(),
                "_".to_string(),
                outdrive.clone(),
                "-b".to_string(),
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

        dosbox_conf_content[i] = format!(
            "imgmount {} sockdrive {}/{}{}",
            drive, url, drive_prefix, outdrive
        );
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

Note 1:
    use '_' as a default preload ranges

Note 2:
    if you use qcow2 image then you must change permissions of /boot/vmlinuz-*
    sudo chmod +r /boot/vmlinuz-*
    
    more:
    https://askubuntu.com/questions/1046828/how-to-run-libguestfs-tools-tools-such-as-virt-make-fs-without-sudo

Example:
    sockdrive mkd win95v1.raw _ ./output [-b]
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
        brotli_all(output_dir);
        reduce_small_files(output_dir, &mut config);
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
                        }
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

fn reduce_small_files(output_dir: &str, metaj: &mut serde_json::Value) {
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

    if small_files.len() > 0 {
        let mut file_locations = Vec::new();
        let mut file_contents = Vec::<u8>::new();

        for path in &small_files {
            let decoded_path = format!("{}/decoded.raw", output_dir);
            let status = Command::new("brotli")
                .arg("-dk")
                .arg(&path)
                .arg("-o")
                .arg(&decoded_path)
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

        let status = Command::new("brotli")
            .arg("-0k")
            .arg(&preload_file_str)
            .status()
            .unwrap();

        if !status.success() {
            eprintln!("Failed to compress preload file");
            std::process::exit(1);
        }

        remove_file(&preload_file_str).unwrap();
        rename(format!("{}/preload.raw.br", output_dir), &preload_file_str).unwrap();
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

        let status = Command::new("brotli")
            .arg("-Zk")
            .arg(&metaj_file)
            .status()
            .unwrap();

        if !status.success() {
            eprintln!("Failed to compress preload file");
            std::process::exit(1);
        }

        remove_file(&metaj_file).unwrap();
        rename(format!("{}/sockdrive.metaj.br", output_dir), metaj_file).unwrap();
    }
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
