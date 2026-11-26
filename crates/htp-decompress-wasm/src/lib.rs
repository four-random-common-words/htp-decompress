//! Raw WebAssembly exports over one [`Decoder`], built once per format.
//!
//! The format is the one enabled feature: `gzip`, `xz` or `zstd`. JavaScript allocates the input and
//! output buffers with [`htp_alloc`], calls [`htp_decoder_process`] and reads the details of the
//! outcome from the [`ResultArea`] that [`htp_result`] points to. A panic or a failed allocation
//! aborts, which traps.

use std::cell::Cell;
use std::{ptr, slice};

use htp_decompress::{Decoder, Error, Format, Limits, Status};

const FORMAT: Format = cfg_select! {
    all(feature = "gzip", not(feature = "xz"), not(feature = "zstd")) => Format::Gzip,
    all(feature = "xz", not(feature = "gzip"), not(feature = "zstd")) => Format::Xz,
    all(feature = "zstd", not(feature = "gzip"), not(feature = "xz")) => Format::Zstd,
    _ => compile_error!("enable exactly one of the features gzip, xz and zstd"),
};

const CONTINUE: i32 = 0;
const DONE: i32 = 1;
const TRUNCATED: i32 = -1;
const CORRUPT: i32 = -2;
const UNSUPPORTED: i32 = -3;
const MEMORY_LIMIT: i32 = -4;
const TRAILING_DATA: i32 = -5;

/// The outcome of the last [`htp_decoder_process`] call, at byte offsets 0, 4, 8, 12 and 16.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ResultArea {
    consumed: u32,
    produced: u32,
    /// The text of a `Corrupt` or `Unsupported` error, else empty.
    message_ptr: u32,
    message_len: u32,
    /// The bytes a `MemoryLimit` error needs, else 0.
    needed: f64,
}

thread_local! {
    static RESULT: Cell<ResultArea> = const {
        Cell::new(ResultArea {
            consumed: 0,
            produced: 0,
            message_ptr: 0,
            message_len: 0,
            needed: 0.0,
        })
    };
}

/// Allocates `len` zeroed bytes; free them with [`htp_free`].
#[unsafe(no_mangle)]
pub extern "C" fn htp_alloc(len: usize) -> *mut u8 {
    Box::into_raw(vec![0u8; len].into_boxed_slice()).cast()
}

/// Frees a buffer from [`htp_alloc`].
///
/// # Safety
///
/// `ptr` and `len` come from one [`htp_alloc`] call whose buffer is not freed yet.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn htp_free(ptr: *mut u8, len: usize) {
    // SAFETY: the caller passes a live allocation of `len` bytes from `htp_alloc`.
    drop(unsafe { Box::from_raw(ptr::slice_from_raw_parts_mut(ptr, len)) });
}

/// Creates a decoder for this build's format.
///
/// `memory_limit` is a whole number of bytes, as a JavaScript number (an `f64`).
#[unsafe(no_mangle)]
pub extern "C" fn htp_decoder_new(memory_limit: f64) -> *mut Decoder {
    let limits = Limits {
        memory: memory_limit as u64,
    };

    Box::into_raw(Box::new(Decoder::new(FORMAT, limits)))
}

/// Frees a decoder from [`htp_decoder_new`].
///
/// # Safety
///
/// `decoder` comes from [`htp_decoder_new`] and is not freed yet.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn htp_decoder_free(decoder: *mut Decoder) {
    // SAFETY: the caller passes a live decoder from `htp_decoder_new`.
    drop(unsafe { Box::from_raw(decoder) });
}

/// Runs one [`Decoder::process`] call and fills the [`ResultArea`].
///
/// Returns 0 to continue, 1 when done, or a negative error code: -1 truncated, -2 corrupt,
/// -3 unsupported, -4 memory limit, -5 trailing data.
///
/// # Safety
///
/// `decoder` comes from [`htp_decoder_new`] and is not freed yet. `input` and `output` point to
/// separate live buffers of at least `input_len` and `output_len` bytes, such as from [`htp_alloc`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn htp_decoder_process(
    decoder: *mut Decoder,
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_len: usize,
    end_of_input: u32,
) -> i32 {
    // SAFETY: the caller passes a live decoder and two separate live buffers of these lengths.
    let (decoder, input, output) = unsafe {
        (
            &mut *decoder,
            slice::from_raw_parts(input, input_len),
            slice::from_raw_parts_mut(output, output_len),
        )
    };

    match decoder.process(input, output, end_of_input != 0) {
        Ok(progress) => {
            report(progress.consumed, progress.produced, "", 0);

            match progress.status {
                Status::Continue => CONTINUE,
                Status::Done => DONE,
            }
        }
        Err(error) => {
            let (code, message, needed) = match error {
                Error::Truncated => (TRUNCATED, "", 0),
                Error::Corrupt(message) => (CORRUPT, message, 0),
                Error::Unsupported(message) => (UNSUPPORTED, message, 0),
                Error::MemoryLimit { needed } => (MEMORY_LIMIT, "", needed),
                Error::TrailingData => (TRAILING_DATA, "", 0),
            };

            report(0, 0, message, needed);

            code
        }
    }
}

/// Points to the [`ResultArea`], which stays at the same address.
#[unsafe(no_mangle)]
pub extern "C" fn htp_result() -> *const ResultArea {
    RESULT.with(|result| result.as_ptr().cast_const())
}

/// Fills the [`ResultArea`].
fn report(consumed: usize, produced: usize, message: &'static str, needed: u64) {
    RESULT.set(ResultArea {
        consumed: consumed as u32,
        produced: produced as u32,
        message_ptr: message.as_ptr() as u32,
        message_len: message.len() as u32,
        needed: needed as f64,
    });
}
