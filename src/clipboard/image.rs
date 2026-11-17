//! Bounded clipboard PNG/DIB conversion. No files, pixels or digests are logged.
use crate::error::{Error, Result};
use std::io::Cursor;
use zeroize::Zeroizing;

pub const MAX_PNG: usize = 4 * 1024 * 1024 - 4096;
pub const MAX_PIXELS: usize = 3840 * 2160;
pub const MAX_DIB: usize = MAX_PIXELS * 4 + 124;
fn invalid(reason: &str) -> Error {
    Error::Clipboard(format!("Clipboard image: {reason}"))
}
fn dimensions(width: u32, height: u32) -> Result<usize> {
    let count = (width as usize)
        .checked_mul(height as usize)
        .filter(|&n| width != 0 && height != 0 && n <= MAX_PIXELS)
        .ok_or_else(|| invalid("dimensions exceed the 4K pixel limit"))?;
    Ok(count)
}
/// Cheap preflight for the control parser; full PNG validation happens off the video thread.
pub fn validate_png(data: &[u8]) -> Result<()> {
    if data.len() < 33
        || data.len() > MAX_PNG
        || &data[..8] != b"\x89PNG\r\n\x1a\n"
        || data[8..12] != 13u32.to_be_bytes()
        || &data[12..16] != b"IHDR"
    {
        return Err(invalid("invalid or oversized PNG"));
    }
    dimensions(
        u32::from_be_bytes(data[16..20].try_into().unwrap()),
        u32::from_be_bytes(data[20..24].try_into().unwrap()),
    )?;
    Ok(())
}
/// Ignore allocation padding after IEND; never send unrelated HGLOBAL bytes.
pub fn png_length(data: &[u8]) -> Result<usize> {
    validate_png(data)?;
    let mut offset = 8usize;
    while offset < data.len() {
        let header = data
            .get(offset..offset + 8)
            .ok_or_else(|| invalid("truncated PNG chunk"))?;
        let size = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
        let end = offset
            .checked_add(size)
            .and_then(|n| n.checked_add(12))
            .filter(|&n| n <= data.len())
            .ok_or_else(|| invalid("invalid PNG chunk length"))?;
        if &header[4..8] == b"IEND" {
            if size != 0 {
                return Err(invalid("invalid IEND"));
            }
            return Ok(end);
        }
        offset = end;
    }
    Err(invalid("missing IEND"))
}
fn read_u32(data: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        data.get(at..at + 4)
            .ok_or_else(|| invalid("truncated DIB"))?
            .try_into()
            .unwrap(),
    ))
}
fn put_u32(data: &mut [u8], at: usize, value: u32) {
    data[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

pub fn dib_to_png(data: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    if data.len() > MAX_DIB {
        return Err(invalid("DIB exceeds limit"));
    }
    let header = read_u32(data, 0)? as usize;
    if !matches!(header, 40 | 108 | 124) || data.len() < header {
        return Err(invalid("unsupported DIB header"));
    }
    let width = read_u32(data, 4)? as i32;
    let height = read_u32(data, 8)? as i32;
    if width <= 0 || height == i32::MIN {
        return Err(invalid("invalid DIB dimensions"));
    }
    let h = height.unsigned_abs();
    let count = dimensions(width as u32, h)?;
    let planes = u16::from_le_bytes(data[12..14].try_into().unwrap());
    let bits = u16::from_le_bytes(data[14..16].try_into().unwrap());
    let compression = read_u32(data, 16)?;
    if planes != 1
        || !matches!(bits, 24 | 32)
        || !matches!(compression, 0 | 3)
        || (compression == 3 && bits != 32)
    {
        return Err(invalid("unsupported DIB pixel encoding"));
    }
    let mut offset = header;
    let alpha = if compression == 3 {
        if read_u32(data, 40)? != 0x00ff0000
            || read_u32(data, 44)? != 0x0000ff00
            || read_u32(data, 48)? != 0x000000ff
        {
            return Err(invalid("unsupported DIB channel masks"));
        }
        if header == 40 {
            offset += 12;
            false
        } else {
            let mask = read_u32(data, 52)?;
            if mask != 0 && mask != 0xff000000 {
                return Err(invalid("unsupported DIB alpha mask"));
            }
            mask != 0
        }
    } else {
        false
    };
    // Palette/profile offsets complicate bounds and are not screenshot pixel data.
    if read_u32(data, 32)? != 0
        || (header == 124 && (read_u32(data, 112)? != 0 || read_u32(data, 116)? != 0))
    {
        return Err(invalid("DIB palettes or embedded profiles are unsupported"));
    }
    let stride = (width as usize * bits as usize).div_ceil(32) * 4;
    let pixels = data
        .get(offset..offset + stride * h as usize)
        .ok_or_else(|| invalid("truncated DIB pixels"))?;
    let mut rgba = Zeroizing::new(vec![0; count * 4]);
    for y in 0..h as usize {
        let source_y = if height > 0 { h as usize - 1 - y } else { y };
        for x in 0..width as usize {
            let source = source_y * stride + x * (bits as usize / 8);
            let target = (y * width as usize + x) * 4;
            rgba[target..target + 4].copy_from_slice(&[
                pixels[source + 2],
                pixels[source + 1],
                pixels[source],
                if alpha { pixels[source + 3] } else { 255 },
            ]);
        }
    }
    let mut out = Zeroizing::new(Vec::new());
    {
        let mut encoder = png::Encoder::new(&mut *out, width as u32, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|_| invalid("PNG header encoding failed"))?;
        writer
            .write_image_data(&rgba)
            .map_err(|_| invalid("PNG encoding failed"))?;
    }
    validate_png(&out)?;
    Ok(out)
}

pub fn png_to_dib(data: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    validate_png(data)?;
    let mut decoder = png::Decoder::new_with_limits(
        Cursor::new(data),
        png::Limits {
            bytes: MAX_PIXELS * 8,
        },
    );
    decoder.set_ignore_text_chunk(true);
    decoder.set_ignore_iccp_chunk(true);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder
        .read_info()
        .map_err(|_| invalid("invalid PNG header"))?;
    if reader.info().animation_control.is_some() {
        return Err(invalid("animated PNG is unsupported"));
    }
    let count = dimensions(reader.info().width, reader.info().height)?;
    let size = reader
        .output_buffer_size()
        .filter(|&size| size <= MAX_PIXELS * 4)
        .ok_or_else(|| invalid("PNG decode exceeds limit"))?;
    let mut pixels = Zeroizing::new(vec![0; size]);
    let info = reader
        .next_frame(&mut pixels)
        .map_err(|_| invalid("invalid PNG pixels"))?;
    reader
        .finish()
        .map_err(|_| invalid("invalid PNG trailer"))?;
    let channels = match info.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Grayscale => 1,
        _ => return Err(invalid("unsupported PNG colors")),
    };
    if info.bit_depth != png::BitDepth::Eight || info.buffer_size() != count * channels {
        return Err(invalid("invalid PNG output size"));
    }
    // BITMAPV5HEADER, top-down BGRA with explicit alpha and sRGB color space.
    let mut out = Zeroizing::new(vec![0; 124 + count * 4]);
    put_u32(&mut out, 0, 124);
    put_u32(&mut out, 4, info.width);
    put_u32(&mut out, 8, (-(info.height as i32)) as u32);
    out[12..14].copy_from_slice(&1u16.to_le_bytes());
    out[14..16].copy_from_slice(&32u16.to_le_bytes());
    put_u32(&mut out, 16, 3);
    put_u32(&mut out, 20, (count * 4) as u32);
    for (at, value) in [
        (40, 0x00ff0000),
        (44, 0x0000ff00),
        (48, 0x000000ff),
        (52, 0xff000000),
        (56, 0x73524742),
        (108, 4),
    ] {
        put_u32(&mut out, at, value);
    }
    for (input, target) in pixels[..info.buffer_size()]
        .chunks_exact(channels)
        .zip(out[124..].chunks_exact_mut(4))
    {
        let (r, g, b, a) = match channels {
            4 => (input[0], input[1], input[2], input[3]),
            3 => (input[0], input[1], input[2], 255),
            2 => (input[0], input[0], input[0], input[1]),
            _ => (input[0], input[0], input[0], 255),
        };
        target.copy_from_slice(&[b, g, r, a]);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn dib() -> Vec<u8> {
        let mut out = vec![0; 48];
        put_u32(&mut out, 0, 40);
        put_u32(&mut out, 4, 2);
        put_u32(&mut out, 8, 1);
        out[12..14].copy_from_slice(&1u16.to_le_bytes());
        out[14..16].copy_from_slice(&24u16.to_le_bytes());
        out[40..46].copy_from_slice(&[0, 0, 255, 0, 255, 0]);
        out
    }
    #[test]
    fn alpha_and_padding_are_preserved_without_sending_allocation_tail() {
        let png = dib_to_png(&dib()).unwrap();
        let mut transparent = png_to_dib(&png).unwrap();
        transparent[127] = 0;
        transparent[131] = 128;
        let png = dib_to_png(&transparent).unwrap();
        let result = png_to_dib(&png).unwrap();
        assert_eq!(&result[124..], &transparent[124..]);
        let mut padded = png.to_vec();
        padded.extend_from_slice(b"unrelated allocation padding");
        assert_eq!(png_length(&padded).unwrap(), png.len());
        assert!(png_length(&png[..png.len() - 5]).is_err());
    }
    #[test]
    fn rgb_round_trip_preserves_colors_and_opaque_alpha() {
        let png = dib_to_png(&dib()).unwrap();
        let result = png_to_dib(&png).unwrap();
        assert_eq!(&result[124..], &[0, 0, 255, 255, 0, 255, 0, 255]);
        assert_eq!(read_u32(&result, 8).unwrap() as i32, -1);
        let again = png_to_dib(&dib_to_png(&result).unwrap()).unwrap();
        assert_eq!(*again, *result);
    }
    #[test]
    fn invalid_dimensions_masks_and_truncation_rejected() {
        for at in [4, 8] {
            let mut data = dib();
            put_u32(&mut data, at, 0x80000000);
            assert!(dib_to_png(&data).is_err());
        }
        assert!(dib_to_png(&dib()[..44]).is_err());
        let mut data = dib();
        put_u32(&mut data, 16, 1);
        assert!(dib_to_png(&data).is_err());
        let mut data = dib();
        put_u32(&mut data, 32, 1);
        assert!(dib_to_png(&data).is_err());
        assert!(png_to_dib(b"not a png").is_err());
        let mut png = dib_to_png(&dib()).unwrap();
        png[20..24].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(png_to_dib(&png).is_err());
    }
    #[test]
    fn corrupt_pixels_or_missing_trailer_rejected() {
        let png = dib_to_png(&dib()).unwrap();
        assert!(png_to_dib(&png[..png.len() - 12]).is_err());
        let mut bad = png.to_vec();
        let n = bad.len();
        bad[n - 15] ^= 1;
        assert!(png_to_dib(&bad).is_err());
    }
    #[test]
    fn bottom_up_dib_padding_and_top_down_order() {
        let mut data = dib();
        data.resize(56, 0);
        put_u32(&mut data, 4, 1);
        put_u32(&mut data, 8, 2);
        data[40..48].copy_from_slice(&[255, 0, 0, 0, 0, 0, 255, 0]);
        let result = png_to_dib(&dib_to_png(&data).unwrap()).unwrap();
        assert_eq!(&result[124..], &[0, 0, 255, 255, 255, 0, 0, 255]);
    }
}

pub const MAX_TIFF: usize = 34 * 1024 * 1024;
/// Restrict the initial IFD before handing data to the decoder's default metadata reader.
pub fn validate_tiff(data: &[u8]) -> Result<()> {
    if data.len() < 8 || data.len() > MAX_TIFF {
        return Err(invalid("TIFF size exceeds limit"));
    }
    let little = match &data[..4] {
        b"II\x2a\0" => true,
        b"MM\0\x2a" => false,
        _ => return Err(invalid("only classic TIFF is supported")),
    };
    let raw: [u8; 4] = data[4..8].try_into().unwrap();
    let offset = if little {
        u32::from_le_bytes(raw)
    } else {
        u32::from_be_bytes(raw)
    } as usize;
    let header = data
        .get(offset..offset + 2)
        .ok_or_else(|| invalid("truncated TIFF directory"))?;
    let count = if little {
        u16::from_le_bytes(header.try_into().unwrap())
    } else {
        u16::from_be_bytes(header.try_into().unwrap())
    } as usize;
    if count > 256 || data.get(offset + 2..offset + 2 + count * 12 + 4).is_none() {
        return Err(invalid("TIFF directory exceeds limit"));
    }
    let read32 = |bytes: &[u8]| {
        let raw: [u8; 4] = bytes.try_into().unwrap();
        if little {
            u32::from_le_bytes(raw)
        } else {
            u32::from_be_bytes(raw)
        }
    };
    let mut metadata = 0usize;
    for entry in data[offset + 2..offset + 2 + count * 12].chunks_exact(12) {
        let kind = if little {
            u16::from_le_bytes(entry[2..4].try_into().unwrap())
        } else {
            u16::from_be_bytes(entry[2..4].try_into().unwrap())
        };
        let unit = match kind {
            1 | 2 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 | 11 | 13 => 4,
            5 | 10 | 12 => 8,
            _ => return Err(invalid("unsupported TIFF field type")),
        };
        let size = (read32(&entry[4..8]) as usize)
            .checked_mul(unit)
            .filter(|&n| n <= 1024 * 1024)
            .ok_or_else(|| invalid("TIFF metadata field exceeds limit"))?;
        metadata += size;
        if metadata > 2 * 1024 * 1024 {
            return Err(invalid("TIFF metadata exceeds limit"));
        }
        if size > 4 {
            let at = read32(&entry[8..12]) as usize;
            if data.get(at..at + size).is_none() {
                return Err(invalid("truncated TIFF metadata"));
            }
        }
    }
    Ok(())
}
pub fn tiff_to_png(data: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    use tiff::{
        ColorType,
        decoder::{Decoder, DecodingResult, Limits},
        tags::Tag,
    };
    validate_tiff(data)?;
    let mut limits = Limits::default();
    limits.decoding_buffer_size = MAX_PIXELS * 4;
    limits.intermediate_buffer_size = MAX_PIXELS * 8;
    limits.ifd_value_size = 1024 * 1024;
    let mut decoder = Decoder::new(Cursor::new(data))
        .map_err(|_| invalid("invalid TIFF metadata"))?
        .with_limits(limits);
    let (width, height) = decoder
        .dimensions()
        .map_err(|_| invalid("missing TIFF dimensions"))?;
    let count = dimensions(width, height)?;
    if decoder.more_images() {
        return Err(invalid("multi-page TIFF is unsupported"));
    }
    let orientation = decoder
        .get_tag_unsigned::<u16>(Tag::Orientation)
        .unwrap_or(1);
    if orientation != 1 {
        return Err(invalid("rotated TIFF is unsupported"));
    }
    let color = decoder
        .colortype()
        .map_err(|_| invalid("invalid TIFF color type"))?;
    let channels = match color {
        ColorType::RGB(8) => 3,
        ColorType::RGBA(8) => 4,
        ColorType::Gray(8) => 1,
        ColorType::GrayA(8) => 2,
        _ => return Err(invalid("only 8-bit RGB/RGBA/gray TIFF is supported")),
    };
    let associated = if matches!(channels, 2 | 4) {
        match decoder.get_tag_u16_vec(Tag::ExtraSamples) {
            Ok(samples) if samples.as_slice() == [1] => true,
            Ok(samples) if samples.as_slice() == [2] => false,
            _ => return Err(invalid("unsupported TIFF alpha semantics")),
        }
    } else {
        false
    };
    let DecodingResult::U8(pixels) = decoder
        .read_image()
        .map_err(|_| invalid("TIFF decoding failed"))?
    else {
        return Err(invalid("unsupported TIFF samples"));
    };
    let pixels = Zeroizing::new(pixels);
    if pixels.len() != count * channels {
        return Err(invalid("TIFF output size mismatch"));
    }
    let mut rgba = Zeroizing::new(vec![0; count * 4]);
    for (pixel, target) in pixels.chunks_exact(channels).zip(rgba.chunks_exact_mut(4)) {
        let (r, g, b, a) = match channels {
            4 => (pixel[0], pixel[1], pixel[2], pixel[3]),
            3 => (pixel[0], pixel[1], pixel[2], 255),
            2 => (pixel[0], pixel[0], pixel[0], pixel[1]),
            _ => (pixel[0], pixel[0], pixel[0], 255),
        };
        let unpremultiply = |v: u8| {
            if associated {
                if a == 0 {
                    0
                } else {
                    ((u32::from(v) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8
                }
            } else {
                v
            }
        };
        target.copy_from_slice(&[unpremultiply(r), unpremultiply(g), unpremultiply(b), a]);
    }
    let mut out = Zeroizing::new(Vec::new());
    {
        let mut encoder = png::Encoder::new(&mut *out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|_| invalid("PNG encoding failed"))?;
        writer
            .write_image_data(&rgba)
            .map_err(|_| invalid("PNG encoding failed"))?;
    }
    validate_png(&out)?;
    Ok(out)
}
#[cfg(test)]
mod tiff_tests {
    use super::*;
    #[test]
    fn common_tiff_compressions_decode_identically() {
        use tiff::encoder::{Compression, DeflateLevel, TiffEncoder, colortype::RGB8};
        for compression in [
            Compression::Uncompressed,
            Compression::Lzw,
            Compression::Deflate(DeflateLevel::default()),
            Compression::Packbits,
        ] {
            let mut cursor = Cursor::new(Vec::new());
            {
                let mut encoder = TiffEncoder::new(&mut cursor)
                    .unwrap()
                    .with_compression(compression);
                encoder
                    .write_image::<RGB8>(2, 1, &[255, 0, 0, 0, 255, 0])
                    .unwrap();
            }
            let dib = png_to_dib(&tiff_to_png(cursor.get_ref()).unwrap()).unwrap();
            assert_eq!(&dib[124..], &[0, 0, 255, 255, 0, 255, 0, 255]);
        }
    }
    #[test]
    fn associated_alpha_is_unpremultiplied_and_rotated_tiff_is_rejected() {
        let mut data = Cursor::new(Vec::new());
        {
            let mut encoder = tiff::encoder::TiffEncoder::new(&mut data).unwrap();
            let mut image = encoder
                .new_image::<tiff::encoder::colortype::RGBA8>(1, 1)
                .unwrap();
            image
                .encoder()
                .write_tag(tiff::tags::Tag::ExtraSamples, &[1u16][..])
                .unwrap();
            image.write_data(&[64, 0, 0, 128]).unwrap();
        }
        let dib = png_to_dib(&tiff_to_png(data.get_ref()).unwrap()).unwrap();
        assert_eq!(&dib[124..], &[0, 0, 128, 128]);
        let mut data = Cursor::new(Vec::new());
        {
            let mut encoder = tiff::encoder::TiffEncoder::new(&mut data).unwrap();
            let mut image = encoder
                .new_image::<tiff::encoder::colortype::RGB8>(1, 1)
                .unwrap();
            image
                .encoder()
                .write_tag(tiff::tags::Tag::Orientation, 3u16)
                .unwrap();
            image.write_data(&[255, 0, 0]).unwrap();
        }
        assert!(tiff_to_png(data.get_ref()).is_err());
    }
    #[test]
    fn rgb_tiff_converts_to_png_with_exact_pixels() {
        let mut data = Cursor::new(Vec::new());
        {
            let mut encoder = tiff::encoder::TiffEncoder::new(&mut data).unwrap();
            encoder
                .write_image::<tiff::encoder::colortype::RGB8>(2, 1, &[255, 0, 0, 0, 255, 0])
                .unwrap();
        }
        let png = tiff_to_png(data.get_ref()).unwrap();
        let dib = png_to_dib(&png).unwrap();
        assert_eq!(&dib[124..], &[0, 0, 255, 255, 0, 255, 0, 255]);
        assert!(tiff_to_png(&data.get_ref()[..10]).is_err());
    }
    #[test]
    fn huge_ifd_and_bigtiff_are_rejected_before_decode() {
        let mut data = vec![b'I', b'I', 42, 0, 8, 0, 0, 0, 255, 255];
        assert!(validate_tiff(&data).is_err());
        data[2] = 43;
        assert!(validate_tiff(&data).is_err());
    }
}
