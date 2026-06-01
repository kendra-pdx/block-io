#![cfg_attr(not(test), no_std)]

mod io_block_reader;

extern crate alloc;

pub use io_block_reader::*;
