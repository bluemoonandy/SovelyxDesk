//! UI-only placement; stable connection settings and credential policy stay untouched.
use serde::{Deserialize, Serialize};
use std::{io::Write, path::Path};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Placement {
    version: u8,
    width: f32,
    height: f32,
    maximized: bool,
}

impl Default for Placement {
    fn default() -> Self {
        Self {
            version: 1,
            width: 1000.0,
            height: 680.0,
            maximized: false,
        }
    }
}

impl Placement {
    fn valid(&self) -> bool {
        self.version == 1
            && (760.0..=7680.0).contains(&self.width)
            && (560.0..=4320.0).contains(&self.height)
    }

    pub(super) fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .filter(|bytes| bytes.len() <= 4096)
            .and_then(|bytes| serde_json::from_slice::<Self>(&bytes).ok())
            .filter(Self::valid)
            .unwrap_or_default()
    }

    pub(super) fn restore(&self, window: &slint::Window) {
        window.set_size(slint::LogicalSize::new(self.width, self.height));
        window.set_maximized(self.maximized);
    }

    pub(super) fn observe(&mut self, window: &slint::Window, immersive: bool) {
        if immersive || window.is_minimized() {
            return;
        }
        let size = window.size().to_logical(window.scale_factor());
        self.record(size.width, size.height, window.is_maximized());
    }

    fn record(&mut self, width: f32, height: f32, maximized: bool) {
        self.maximized = maximized;
        if !maximized {
            let candidate = Self {
                width,
                height,
                ..*self
            };
            if candidate.valid() {
                *self = candidate;
            }
        }
    }

    pub(super) fn save(&self, path: &Path) -> std::io::Result<()> {
        let parent = path.parent().ok_or(std::io::ErrorKind::InvalidInput)?;
        std::fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(&serde_json::to_vec(self)?)?;
        file.as_file().sync_all()?;
        file.persist(path).map_err(|error| error.error)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maximize_preserves_last_normal_size_for_restore() {
        let mut placement = Placement::default();
        placement.record(1200.0, 800.0, false);
        placement.record(1920.0, 1080.0, true);
        assert!(placement.maximized);
        assert_eq!((placement.width, placement.height), (1200.0, 800.0));
        placement.record(1200.0, 800.0, false);
        assert!(!placement.maximized);
    }

    #[test]
    fn invalid_or_partial_sizes_do_not_replace_normal_placement() {
        let mut placement = Placement::default();
        for (width, height) in [(0.0, 0.0), (f32::NAN, 800.0), (1000.0, 50.0)] {
            placement.record(width, height, false);
        }
        assert_eq!((placement.width, placement.height), (1000.0, 680.0));
    }

    #[test]
    fn placement_round_trip_is_independent_of_connection_settings() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ui-window-v1.json");
        let mut placement = Placement::default();
        placement.record(1440.0, 900.0, false);
        placement.save(&path).unwrap();
        let restored = Placement::load(&path);
        assert_eq!((restored.width, restored.height), (1440.0, 900.0));
        assert!(!directory.path().join("settings.json").exists());
        std::fs::write(&path, b"partial JSON").unwrap();
        assert_eq!(Placement::load(&path).width, 1000.0);
    }
}
