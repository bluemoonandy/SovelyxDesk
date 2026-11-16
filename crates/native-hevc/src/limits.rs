use crate::{Error, Result};

pub const MAX_DIMENSION: u32 = 8192;
pub const MAX_PIXELS: u64 = 16_777_216;
pub const MAX_INPUT_BYTES: usize = 64 * 1024 * 1024;

pub(crate) fn validate_dimensions(width: u32, height: u32) -> Result<()> {
    if width == 0
        || height == 0
        || width > MAX_DIMENSION
        || height > MAX_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_PIXELS
    {
        Err(Error::InvalidDimensions { width, height })
    } else {
        Ok(())
    }
}
