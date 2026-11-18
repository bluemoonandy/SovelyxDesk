use crate::error::{Error, Result};

pub const MAX_DIMENSION: u32 = 16_384;
pub const MAX_PIXELS: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Extent {
    pub width: u32,
    pub height: u32,
}

impl Extent {
    pub fn new(width: u32, height: u32) -> Result<Self> {
        if width == 0
            || height == 0
            || width > MAX_DIMENSION
            || height > MAX_DIMENSION
            || u64::from(width) * u64::from(height) > MAX_PIXELS
        {
            return Err(Error::Protocol("Unsupported framebuffer dimensions".into()));
        }
        Ok(Self { width, height })
    }

    pub fn bytes(self) -> usize {
        self.width as usize * self.height as usize * 4
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub fn validate(self, extent: Extent) -> Result<()> {
        if self.width == 0
            || self.height == 0
            || self
                .x
                .checked_add(self.width)
                .is_none_or(|x| x > extent.width)
            || self
                .y
                .checked_add(self.height)
                .is_none_or(|y| y > extent.height)
        {
            return Err(Error::Protocol("Rectangle exceeds the framebuffer".into()));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ViewScale {
    #[default]
    Fit,
    Actual,
    Zoom(u16),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub source: Extent,
    // Draw rectangle and input positions use Slint logical pixels.
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
    // Physical output size for CPU resampling; GPU images keep their decoded size.
    pub pixels: Extent,
    dpi: f64,
    canvas: (f32, f32),
}

impl Viewport {
    pub fn fit(source: Extent, width: f32, height: f32, dpi: f32) -> Result<Self> {
        Self::scaled(source, width, height, dpi, ViewScale::Fit, (0.0, 0.0))
    }

    pub fn scaled(
        source: Extent,
        width: f32,
        height: f32,
        dpi: f32,
        scale: ViewScale,
        pan: (f32, f32),
    ) -> Result<Self> {
        if !width.is_finite()
            || !height.is_finite()
            || !dpi.is_finite()
            || width <= 0.0
            || height <= 0.0
            || dpi <= 0.0
        {
            return Err(Error::Display("Invalid canvas dimensions or DPI".into()));
        }
        let dpi = f64::from(dpi);
        let factor = (f64::from(width) * dpi / f64::from(source.width))
            .min(f64::from(height) * dpi / f64::from(source.height));
        let factor = match scale {
            ViewScale::Fit => factor,
            ViewScale::Actual => 1.0,
            ViewScale::Zoom(percent) if matches!(percent, 100 | 125 | 150 | 175 | 200) => {
                factor * f64::from(percent) / 100.0
            }
            ViewScale::Zoom(_) => return Err(Error::Display("Unsupported display zoom".into())),
        };
        if !pan.0.is_finite() || !pan.1.is_finite() {
            return Err(Error::Display("Invalid display pan".into()));
        }
        let pixels = Extent::new(
            (f64::from(source.width) * factor).floor().max(1.0) as u32,
            (f64::from(source.height) * factor).floor().max(1.0) as u32,
        )?;
        let draw_width = (f64::from(pixels.width) / dpi) as f32;
        let draw_height = (f64::from(pixels.height) / dpi) as f32;
        Ok(Self {
            source,
            left: (width - draw_width) / 2.0
                - pan.0.clamp(-1.0, 1.0) * (draw_width - width).max(0.0) / 2.0,
            top: (height - draw_height) / 2.0
                - pan.1.clamp(-1.0, 1.0) * (draw_height - height).max(0.0) / 2.0,
            width: draw_width,
            height: draw_height,
            pixels,
            dpi,
            canvas: (width, height),
        })
    }

    /// Dragging the view moves content with the pointer; normalized offsets survive DPI changes.
    pub fn pan_after_drag(self, pan: (f32, f32), dx: f32, dy: f32) -> (f32, f32) {
        let shift = |current: f32, delta: f32, overflow: f32| {
            if overflow > 0.0 && delta.is_finite() {
                (current - 2.0 * delta / overflow).clamp(-1.0, 1.0)
            } else {
                0.0
            }
        };
        (
            shift(pan.0, dx, self.width - self.canvas.0),
            shift(pan.1, dy, self.height - self.canvas.1),
        )
    }

    pub fn to_remote(self, x: f32, y: f32, dragging: bool) -> Option<(u16, u16)> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        if !dragging
            && (x < 0.0
                || y < 0.0
                || x >= self.canvas.0
                || y >= self.canvas.1
                || x < self.left
                || y < self.top
                || x >= self.left + self.width
                || y >= self.top + self.height)
        {
            return None;
        }
        // Captured remote drags may leave the canvas. Clamp to the visible view,
        // including when Zoom crops a larger source image behind the canvas.
        let (x, y) = if dragging {
            (
                x.clamp(0.0, (self.canvas.0 - 1.0 / self.dpi as f32).max(0.0)),
                y.clamp(0.0, (self.canvas.1 - 1.0 / self.dpi as f32).max(0.0)),
            )
        } else {
            (x, y)
        };
        let map = |position: f32, origin: f32, pixels: u32, source: u32| -> u16 {
            let pixel = (f64::from(position) - f64::from(origin)) * self.dpi;
            if pixel <= 0.0 {
                return 0;
            }
            if pixel >= f64::from(pixels - 1) {
                return (source - 1) as u16;
            }
            (((pixel + 0.5) * f64::from(source) / f64::from(pixels)).floor() as u32).min(source - 1)
                as u16
        };
        Some((
            map(x, self.left, self.pixels.width, self.source.width),
            map(y, self.top, self.pixels.height, self.source.height),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_four_k_on_1440p() {
        let view = Viewport::fit(Extent::new(3840, 2160).unwrap(), 2560.0, 1440.0, 1.0).unwrap();
        assert_eq!(view.pixels, Extent::new(2560, 1440).unwrap());
        assert_eq!(view.to_remote(1280.0, 720.0, false), Some((1920, 1080)));
        assert_eq!(view.to_remote(2559.0, 1439.0, false), Some((3839, 2159)));
        assert_eq!(view.to_remote(0.0, 0.0, false), Some((0, 0)));
    }

    #[test]
    fn maps_real_windows_dpi_coordinates() {
        let view = Viewport::fit(Extent::new(3840, 2160).unwrap(), 1280.0, 720.0, 1.5).unwrap();
        assert_eq!(view.pixels, Extent::new(1920, 1080).unwrap());
        assert_eq!(view.to_remote(640.0, 360.0, false), Some((1921, 1081)));
        assert_eq!(
            view.to_remote(1279.3334, 719.3334, false),
            Some((3839, 2159))
        );
    }

    #[test]
    fn padding_does_not_click_the_mac() {
        let view = Viewport::fit(Extent::new(3840, 2160).unwrap(), 900.0, 800.0, 1.0).unwrap();
        assert_eq!(view.pixels, Extent::new(900, 506).unwrap());
        assert_eq!(view.to_remote(50.0, 10.0, false), None);
        assert_eq!(view.to_remote(899.0, 652.0, false), Some((3839, 2159)));
        assert_eq!(view.to_remote(-50.0, 1000.0, true), Some((0, 2159)));
    }

    #[test]
    fn smaller_desktops_fill_the_available_canvas() {
        let view = Viewport::fit(Extent::new(1280, 720).unwrap(), 2560.0, 1440.0, 1.0).unwrap();
        assert_eq!(view.pixels, Extent::new(2560, 1440).unwrap());
        assert_eq!(view.left, 0.0);
    }

    #[test]
    fn fit_uses_the_canvas_at_every_dpi_and_aspect_ratio() {
        for source in [(1280, 720), (3840, 2160), (1920, 1200), (1600, 1200)] {
            let source = Extent::new(source.0, source.1).unwrap();
            for (physical_width, physical_height) in
                [(640.0, 480.0), (2880.0, 1800.0), (3840.0, 2160.0)]
            {
                for dpi in [1.0, 1.5, 2.0] {
                    let width = physical_width / dpi;
                    let height = physical_height / dpi;
                    let view = Viewport::fit(source, width, height, dpi).unwrap();
                    let scale = (physical_width / source.width as f32)
                        .min(physical_height / source.height as f32);
                    assert!((view.width * dpi - source.width as f32 * scale).abs() < 1.01);
                    assert!((view.height * dpi - source.height as f32 * scale).abs() < 1.01);
                    assert!(view.left >= 0.0 && view.top >= 0.0);
                    assert!((view.left * 2.0 + view.width - width).abs() < 0.001);
                    assert!((view.top * 2.0 + view.height - height).abs() < 0.001);
                    assert!(
                        view.to_remote(view.left + view.width, view.top, false)
                            .is_none()
                    );
                    assert!(
                        view.to_remote(view.left, view.top + view.height, false)
                            .is_none()
                    );
                    assert_eq!(view.to_remote(view.left, view.top, false), Some((0, 0)));
                    assert_eq!(view.to_remote(-1.0, -1.0, false), None);
                    assert_eq!(view.to_remote(-1.0, -1.0, true), Some((0, 0)));
                    assert_eq!(
                        view.to_remote(width + 1.0, height + 1.0, true),
                        Some(((source.width - 1) as u16, (source.height - 1) as u16))
                    );
                }
            }
        }
    }

    #[test]
    fn resizing_fullscreen_and_cross_monitor_moves_use_fresh_geometry() {
        let source = Extent::new(1280, 720).unwrap();
        let initial = Viewport::fit(source, 960.0, 600.0, 2.0).unwrap();
        let mut previous = initial;
        // Window, maximized laptop, full screen, external 4K, then back to window.
        for (width, height, dpi) in [
            (1440.0, 780.0, 2.0),
            (1440.0, 900.0, 2.0),
            (3840.0, 2160.0, 1.0),
            (1920.0, 1080.0, 2.0),
            (960.0, 600.0, 2.0),
        ] {
            let next = Viewport::fit(source, width, height, dpi).unwrap();
            assert_ne!(next, previous);
            let center = next.to_remote(width / 2.0, height / 2.0, false).unwrap();
            assert!((i32::from(center.0) - 640).abs() <= 1);
            assert!((i32::from(center.1) - 360).abs() <= 1);
            previous = next;
        }
        assert_eq!(previous, initial);
    }

    #[test]
    fn zoom_enlarges_fit_at_every_dpi_without_changing_source() {
        let source = Extent::new(1920, 1080).unwrap();
        for dpi in [1.0, 1.5, 2.0] {
            let fit = Viewport::fit(source, 1440.0, 900.0, dpi).unwrap();
            for percent in [100, 125, 150, 175, 200] {
                let zoom = Viewport::scaled(
                    source,
                    1440.0,
                    900.0,
                    dpi,
                    ViewScale::Zoom(percent),
                    (0.0, 0.0),
                )
                .unwrap();
                assert_eq!(zoom.source, source);
                assert!((zoom.width - fit.width * percent as f32 / 100.0).abs() < 1.1 / dpi);
                assert!((zoom.height - fit.height * percent as f32 / 100.0).abs() < 1.1 / dpi);
                let center = zoom.to_remote(720.0, 450.0, false).unwrap();
                assert!((i32::from(center.0) - 960).abs() <= 1);
                assert!((i32::from(center.1) - 540).abs() <= 1);
            }
        }
    }

    #[test]
    fn actual_size_is_one_source_pixel_per_physical_pixel() {
        let source = Extent::new(1920, 1080).unwrap();
        for dpi in [1.0, 1.5, 2.0] {
            let view = Viewport::scaled(source, 1440.0, 900.0, dpi, ViewScale::Actual, (0.0, 0.0))
                .unwrap();
            assert_eq!(view.pixels, source);
            assert!((view.width * dpi - 1920.0).abs() < 0.001);
        }
    }

    #[test]
    fn pan_changes_mapping_and_never_exposes_empty_space() {
        let source = Extent::new(1920, 1080).unwrap();
        let center =
            Viewport::scaled(source, 1000.0, 600.0, 2.0, ViewScale::Zoom(200), (0.0, 0.0)).unwrap();
        let top_left = Viewport::scaled(
            source,
            1000.0,
            600.0,
            2.0,
            ViewScale::Zoom(200),
            (-1.0, -1.0),
        )
        .unwrap();
        let bottom_right =
            Viewport::scaled(source, 1000.0, 600.0, 2.0, ViewScale::Zoom(200), (1.0, 1.0)).unwrap();
        assert_eq!((top_left.left, top_left.top), (0.0, 0.0));
        assert_eq!(bottom_right.left + bottom_right.width, 1000.0);
        assert_eq!(bottom_right.top + bottom_right.height, 600.0);
        assert!(
            bottom_right.to_remote(500.0, 300.0, false).unwrap().0
                > center.to_remote(500.0, 300.0, false).unwrap().0
        );
        assert_eq!(bottom_right.to_remote(1000.0, 300.0, false), None);
        assert_eq!(center.to_remote(-1.0, 300.0, false), None);
        assert!(center.to_remote(-1.0, 300.0, true).is_some());
        assert_eq!(
            center.to_remote(-100.0, 300.0, true),
            center.to_remote(0.0, 300.0, true)
        );
        assert_eq!(
            center.to_remote(2000.0, 300.0, true),
            center.to_remote(999.5, 300.0, true)
        );
    }

    #[test]
    fn local_drag_pan_is_bounded_and_fit_has_no_pan() {
        let source = Extent::new(1920, 1080).unwrap();
        let fit = Viewport::fit(source, 1000.0, 600.0, 2.0).unwrap();
        assert_eq!(fit.pan_after_drag((0.0, 0.0), 100.0, 100.0), (0.0, 0.0));
        let zoom =
            Viewport::scaled(source, 1000.0, 600.0, 2.0, ViewScale::Zoom(200), (0.0, 0.0)).unwrap();
        assert_eq!(
            zoom.pan_after_drag((0.0, 0.0), 5000.0, -5000.0),
            (-1.0, 1.0)
        );
        assert!(
            Viewport::scaled(source, 1000.0, 600.0, 2.0, ViewScale::Zoom(999), (0.0, 0.0)).is_err()
        );
        assert!(
            Viewport::scaled(source, 1000.0, 600.0, 2.0, ViewScale::Fit, (f32::NAN, 0.0)).is_err()
        );
    }

    #[test]
    fn rejects_unbounded_allocations_and_invalid_dpi() {
        assert!(Extent::new(0, 2160).is_err());
        assert!(Extent::new(65_535, 65_535).is_err());
        assert!(Extent::new(16_384, 16_384).is_err());
        assert!(Viewport::fit(Extent::new(4, 4).unwrap(), 100.0, 100.0, 0.0).is_err());
    }
}
