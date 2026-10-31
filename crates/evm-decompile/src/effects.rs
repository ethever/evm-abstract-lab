//! Conservative per-primitive state scopes. No storage/world read is pure.
use crate::Effects;

pub(crate) fn effects(op: u8) -> Effects {
    let mut e = Effects::default();
    match op {
        0x20 | 0x51 => e.memory_read = true,
        0x52 | 0x53 | 0x37 | 0x39 => e.memory_write = true,
        0x3c => {
            e.memory_write = true;
            e.world_read = true;
        }
        0x3e => {
            e.memory_write = true;
            e.return_data_read = true;
        }
        0x3d => e.return_data_read = true,
        0x54 => e.storage_read = true,
        0x55 => e.storage_write = true,
        0x5c => e.transient_read = true,
        0x5d => e.transient_write = true,
        0x5e => {
            e.memory_read = true;
            e.memory_write = true;
        }
        0xa0..=0xa4 => {
            e.memory_read = true;
            e.log = true;
        }
        0xf0 | 0xf5 => {
            e.memory_read = true;
            e.storage_read = true;
            e.storage_write = true;
            e.transient_read = true;
            e.transient_write = true;
            e.world_read = true;
            e.world_write = true;
            e.create = true;
            e.return_data_write = true;
        }
        0xf1 | 0xf2 | 0xf4 | 0xfa => {
            e.memory_read = true;
            e.memory_write = true;
            e.storage_read = true;
            e.storage_write = op != 0xfa;
            e.transient_read = true;
            e.transient_write = op != 0xfa;
            e.world_read = true;
            e.world_write = op != 0xfa;
            e.call = true;
            e.return_data_write = true;
        }
        0xf3 | 0xfd => {
            e.memory_read = true;
            e.terminate = true;
        }
        0xff => {
            e.world_read = true;
            e.world_write = true;
            e.terminate = true;
        }
        0x00 | 0xfe => e.terminate = true,
        // Inputs are explicit observations. GAS/MSIZE vary with preceding work.
        0x30..=0x36 | 0x38 | 0x3a | 0x3b | 0x3f..=0x4a | 0x58..=0x5a => e.world_read = true,
        _ => {}
    }
    e
}
