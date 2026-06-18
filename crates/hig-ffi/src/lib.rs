//! Reserved C++ codec boundary for future high-performance native extensions.
//!
//! V1 keeps the production path in safe Rust. This crate records the ABI shape
//! expected from future C++ codecs without linking or loading one by default.

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HigCodecStatus {
    Ok = 0,
    InvalidInput = 1,
    OutputTooSmall = 2,
    InternalError = 255,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct HigBuffer {
    pub ptr: *const u8,
    pub len: usize,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct HigMutableBuffer {
    pub ptr: *mut u8,
    pub len: usize,
}

pub type HigCompressFn = unsafe extern "C" fn(
    input: HigBuffer,
    output: HigMutableBuffer,
    written: *mut usize,
    level: i32,
) -> HigCodecStatus;

pub type HigDecompressFn = unsafe extern "C" fn(
    input: HigBuffer,
    output: HigMutableBuffer,
    written: *mut usize,
) -> HigCodecStatus;
