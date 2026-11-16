use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

/// Allocate independent storage that GPU readback must fully overwrite before submission.
pub fn readback_buffer(
    width: u32,
    height: u32,
    previous: Option<&Image>,
) -> SharedPixelBuffer<Rgba8Pixel> {
    // In Slint 1.18, the slice constructor avoids the per-pixel iterator bookkeeping
    // of `new`. Copying the previous CPU image provides initialized storage without
    // another resident template. Never mutate that image: the renderer may own it.
    if let Some(previous) = previous.and_then(Image::to_rgba8)
        && previous.width() == width
        && previous.height() == height
    {
        return SharedPixelBuffer::clone_from_slice(previous.as_bytes(), width, height);
    }
    SharedPixelBuffer::new(width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readback_does_not_change_a_submitted_image() {
        let mut source = SharedPixelBuffer::<Rgba8Pixel>::new(4, 2);
        source.make_mut_bytes().fill(37);
        let previous = Image::from_rgba8(source);
        let displayed = previous.clone();
        let mut next = readback_buffer(4, 2, Some(&previous));
        assert_eq!(next.as_bytes(), &[37; 32]);
        assert_ne!(
            next.as_bytes().as_ptr(),
            previous.to_rgba8().unwrap().as_bytes().as_ptr()
        );
        next.make_mut_bytes().fill(91);
        let submitted = Image::from_rgba8(next);
        assert_eq!(displayed.to_rgba8().unwrap().as_bytes(), &[37; 32]);
        assert_eq!(submitted.to_rgba8().unwrap().as_bytes(), &[91; 32]);
    }

    #[test]
    fn resize_with_equal_byte_count_uses_new_dimensions() {
        let mut source = SharedPixelBuffer::<Rgba8Pixel>::new(4, 2);
        source.make_mut_bytes().fill(37);
        let previous = Image::from_rgba8(source);
        let next = readback_buffer(2, 4, Some(&previous));
        assert_eq!((next.width(), next.height()), (2, 4));
        assert_eq!(next.as_bytes(), &[0; 32]);
        let larger = readback_buffer(4, 3, Some(&previous));
        assert_eq!(larger.as_bytes().len(), 48);
    }

    #[test]
    fn first_frame_and_unavailable_pixels_use_initialized_storage() {
        for previous in [None, Some(Image::default())] {
            let next = readback_buffer(3, 2, previous.as_ref());
            assert_eq!((next.width(), next.height()), (3, 2));
            assert_eq!(next.as_bytes(), &[0; 24]);
        }
    }
}
