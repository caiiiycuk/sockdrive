use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::{
    fs::File,
    io::{
        Cursor, Read, Seek,
        SeekFrom::{Current, End, Start},
        Write,
    },
    mem::size_of,
};

pub const SECTOR_SIZE: usize = 512;
pub const SECTOR_SIZE_64: u64 = SECTOR_SIZE as u64;
pub const NO_VALUE: u32 = std::u32::MAX;
pub const NO_VALUE_64: u64 = NO_VALUE as u64;

pub struct Layer {
    meta_file: String,
    blob: File,
    offsets: Vec<u32>,
    pos: u64,
}

impl Layer {
    pub fn new(name: &str, sectors: usize) -> Self {
        let meta_file = name.to_owned() + "-meta";
        let mut offsets = vec![NO_VALUE; sectors];
        if let Ok(mut meta) = File::open(&meta_file) {
            let mut header: Vec<u8> = vec![0; sectors * size_of::<u32>()];
            meta.read_exact(&mut header)
                .expect("Unable to read header from meta file");

            let mut cursor = Cursor::new(&header);
            for i in 0..sectors {
                offsets[i] = cursor
                    .read_u32::<LittleEndian>()
                    .expect("Wrong header structure");
            }
        }

        let blob_file = name.to_owned() + "-blob";
        let mut blob = File::options()
            .create(true)
            .read(true)
            .write(true)
            .open(blob_file)
            .expect("Unable to create blob file");
        let pos = blob.seek(Current(0)).unwrap();
        Layer {
            meta_file,
            blob,
            offsets,
            pos,
        }
    }

    pub fn read(&mut self, sector: u32, buffer: &mut [u8; SECTOR_SIZE]) {
        let offset = self.offsets[sector as usize] as u64;
        if offset == NO_VALUE_64 {
            buffer.fill(0);
        } else {
            if self.pos != offset {
                self.blob.seek(Start(offset)).expect("Can't seek to sector");
            }
            self.blob.read_exact(buffer).expect("Can't read sector");
            self.pos += offset + SECTOR_SIZE_64;
        }
    }

    pub fn write(&mut self, sector: u32, buffer: &[u8; SECTOR_SIZE]) {
        let mut offset = self.offsets[sector as usize] as u64;
        if offset == NO_VALUE_64 {
            offset = self.blob.seek(End(0)).expect("Can't seek to the end");
            self.pos = offset;
            self.offsets[sector as usize] = offset as u32;
        }
        if self.pos != offset {
            self.blob.seek(Start(offset)).expect("Can't seek to sector");
        }
        self.blob.write_all(buffer).expect("Can't write sector");
        self.pos = offset + SECTOR_SIZE_64;
    }

    pub fn flush(&mut self) {
        let mut meta = File::options()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.meta_file)
            .expect("Unable to create/overwrite meta file");

        for i in 0..self.offsets.len() {
            meta.write_u32::<LittleEndian>(self.offsets[i])
                .expect("Unable to write meta file");
        }
        meta.flush().expect("Unable to flush meta file");
    }
}

#[cfg(test)]
mod tests {
    use crate::layer::NO_VALUE;

    use super::{Layer, SECTOR_SIZE};

    #[test]
    fn create_write_read_test() {
        let layer_file = "create-write-read-test";
        let meta_file = layer_file.to_owned() + "-meta";
        let blob_file = layer_file.to_owned() + "-blob";
        let _ = std::fs::remove_file(&meta_file);
        let _ = std::fs::remove_file(&blob_file);

        {
            let mut layer = Layer::new(layer_file, 16);
            let mut buffer: [u8; SECTOR_SIZE] = [0; SECTOR_SIZE];
            for i in 0..16 {
                assert_eq!(layer.offsets[i as usize], NO_VALUE);
                buffer[0] = i as u8;
                layer.write(i, &buffer);
                assert_eq!(layer.offsets[i as usize], i * SECTOR_SIZE as u32);
            }
            layer.flush();
        }

        {
            let mut layer = Layer::new(layer_file, 16);
            let mut buffer: [u8; SECTOR_SIZE] = [0; SECTOR_SIZE];
            for i in 0..16 {
                assert_eq!(layer.offsets[i as usize], i * SECTOR_SIZE as u32);
                layer.read(i, &mut buffer);
                assert_eq!(buffer[0], i as u8);
            }
            layer.flush();
        }

        let _ = std::fs::remove_file(&meta_file);
        let _ = std::fs::remove_file(&blob_file);
    }
}
