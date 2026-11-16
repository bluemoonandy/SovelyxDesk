// Compile the production parser without the Windows decoder, UI, or native build scripts.
// rustc --edition=2024 --test scripts/test_hevc_bitstream.rs -o <temporary test binary>
#[path = "../crates/native-hevc/src/annex_b.rs"]
pub mod annex_b;
#[path = "../crates/native-hevc/src/error.rs"]
mod error;
#[path = "../crates/native-hevc/src/limits.rs"]
mod limits;

pub use error::{Error, Result};
use limits::validate_dimensions;
pub use limits::{MAX_DIMENSION, MAX_INPUT_BYTES, MAX_PIXELS};
