use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
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

pub struct Layer {
    file: File,
    offsets: Vec<u32>,
    pos: u64,
}

impl Layer {
    pub fn new(file: &str, sectors: usize) -> Self {
        let mut offsets = vec![0; sectors as usize];
        let mut file = File::options()
            .create(true)
            .read(true)
            .write(true)
            .open(file)
            .unwrap();

        if file.metadata().unwrap().len() > 0 {
            file.seek(Start(0)).unwrap();
            let mut header: Vec<u8> = vec![0; sectors * size_of::<u32>()];
            file.read_exact(&mut header).unwrap();

            let mut cursor = Cursor::new(&header);
            for i in 0..sectors {
                offsets[i] = cursor.read_u32::<BigEndian>().unwrap();
            }
        }

        let pos = file.seek(Current(0)).unwrap();
        Layer { file, offsets, pos }
    }

    pub fn read(&mut self, sector: u32, buffer: &mut [u8; SECTOR_SIZE]) {
        let offset = self.offsets[sector as usize] as u64;
        if offset == 0 {
            buffer.fill(0);
        } else {
            if self.pos != offset {
                self.file.seek(Start(offset)).unwrap();
            }
            self.file.read_exact(buffer).unwrap();
            self.pos += offset + SECTOR_SIZE as u64;
        }
    }

    pub fn write(&mut self, sector: u32, buffer: &[u8; SECTOR_SIZE]) {
        let mut offset = self.offsets[sector as usize] as u64;
        if offset == 0 {
            offset = self.file.seek(End(0)).unwrap();
            self.pos = offset;
            self.offsets[sector as usize] = offset as u32;
        }
        if self.pos != offset {
            self.file.seek(Start(offset)).unwrap();
        }
        self.file.write_all(buffer).unwrap();
        self.pos = offset + SECTOR_SIZE as u64;
    }

    pub fn flush(&mut self) {
        self.file.seek(Start(0)).unwrap();
        for i in 0..self.offsets.len() {
            self.file.write_u32::<BigEndian>(self.offsets[i]).unwrap();
        }
        self.file.flush().unwrap()
    }
}
