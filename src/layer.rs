pub const SECTOR_SIZE: usize = 512;
pub const SECTOR_SIZE_64: u64 = SECTOR_SIZE as u64;

pub trait Layer {
    fn read(&mut self, sector: u32, buffer: &mut [u8; SECTOR_SIZE]);
    fn write(&mut self, sector: u32, buffer: &[u8; SECTOR_SIZE]);
    fn flush(&mut self);
}

pub mod disk_layer;