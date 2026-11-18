//! Display-only per-device preferences; no connection or credential settings are rewritten.
use maccanvas::{
    error::{Error, Result},
    geometry::ViewScale,
};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, io::Write, path::PathBuf};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Mode {
    #[default]
    Fit,
    Actual,
    Zoom(u16),
}
impl Mode {
    pub(super) fn from_index(index: i32) -> Option<Self> {
        match index {
            0 => Some(Self::Fit),
            1 => Some(Self::Actual),
            2..=6 => Some(Self::Zoom(100 + (index as u16 - 2) * 25)),
            _ => None,
        }
    }
    pub(super) fn index(self) -> i32 {
        match self {
            Self::Fit => 0,
            Self::Actual => 1,
            Self::Zoom(percent) => 2 + i32::from(percent - 100) / 25,
        }
    }
    pub(super) fn scale(self) -> ViewScale {
        match self {
            Self::Fit => ViewScale::Fit,
            Self::Actual => ViewScale::Actual,
            Self::Zoom(percent) => ViewScale::Zoom(percent),
        }
    }
    fn valid(self) -> bool {
        matches!(
            self,
            Self::Fit | Self::Actual | Self::Zoom(100 | 125 | 150 | 175 | 200)
        )
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct View {
    pub mode: Mode,
    pub pan: (f32, f32),
}
impl View {
    fn valid(self) -> bool {
        self.mode.valid()
            && self.pan.0.is_finite()
            && self.pan.1.is_finite()
            && (-1.0..=1.0).contains(&self.pan.0)
            && (-1.0..=1.0).contains(&self.pan.1)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Device {
    id: Uuid,
    view: View,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Data {
    version: u8,
    devices: Vec<Device>,
}
impl Default for Data {
    fn default() -> Self {
        Self {
            version: 1,
            devices: Vec::new(),
        }
    }
}
pub(super) struct Store {
    path: PathBuf,
    data: Data,
    load_error: Option<String>,
}
impl Store {
    pub(super) fn load(path: PathBuf) -> Self {
        let read = || -> std::result::Result<Data, String> {
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(Data::default());
                }
                Err(error) => return Err(error.to_string()),
            };
            if bytes.len() > 128 * 1024 {
                return Err("Display preferences exceed the size limit".into());
            }
            let data: Data = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
            let mut ids = HashSet::new();
            if data.version != 1
                || data.devices.len() > 128
                || data
                    .devices
                    .iter()
                    .any(|d| !d.view.valid() || !ids.insert(d.id))
            {
                return Err("Invalid display preferences".into());
            }
            Ok(data)
        };
        let (data, load_error) = match read() {
            Ok(data) => (data, None),
            Err(error) => (Data::default(), Some(error)),
        };
        Self {
            path,
            data,
            load_error,
        }
    }
    pub(super) fn view(&self, id: Uuid) -> View {
        self.data
            .devices
            .iter()
            .find(|d| d.id == id)
            .map(|d| d.view)
            .unwrap_or_default()
    }
    pub(super) fn set(&mut self, id: Uuid, view: View) -> Result<()> {
        if !view.valid() {
            return Err(Error::Configuration("Invalid display zoom or pan".into()));
        }
        if let Some(device) = self.data.devices.iter_mut().find(|d| d.id == id) {
            device.view = view;
        } else if self.data.devices.len() < 128 {
            self.data.devices.push(Device { id, view });
        } else {
            return Err(Error::Configuration(
                "Display preference device limit reached".into(),
            ));
        }
        Ok(())
    }
    pub(super) fn save(&self) -> Result<()> {
        if let Some(error) = &self.load_error {
            return Err(Error::Configuration(format!(
                "Original display preferences were preserved: {error}"
            )));
        }
        let parent = self
            .path
            .parent()
            .ok_or_else(|| Error::Configuration("Missing display preferences directory".into()))?;
        std::fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer(&mut temporary, &self.data)
            .map_err(|e| Error::Configuration(e.to_string()))?;
        temporary.flush()?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(&self.path)
            .map_err(|e| Error::Configuration(e.to_string()))?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn devices_restore_independent_zoom_and_pan_without_touching_login() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("display-v1.json");
        let login = dir.path().join("connections-v1.json");
        std::fs::write(&login, b"preserve original").unwrap();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let mut store = Store::load(path.clone());
        store
            .set(
                a,
                View {
                    mode: Mode::Zoom(150),
                    pan: (0.5, -1.0),
                },
            )
            .unwrap();
        store
            .set(
                b,
                View {
                    mode: Mode::Actual,
                    pan: (0.0, 0.0),
                },
            )
            .unwrap();
        store.save().unwrap();
        let loaded = Store::load(path);
        assert_eq!(loaded.view(a), store.view(a));
        assert_eq!(loaded.view(b), store.view(b));
        assert_eq!(loaded.view(Uuid::new_v4()), View::default());
        assert_eq!(std::fs::read(login).unwrap(), b"preserve original");
        for index in 0..=6 {
            assert_eq!(Mode::from_index(index).unwrap().index(), index);
        }
    }
    #[test]
    fn corrupt_or_unsupported_preferences_are_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("display-v1.json");
        std::fs::write(&path, b"broken").unwrap();
        let store = Store::load(path.clone());
        assert!(store.save().is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"broken");
        assert!(
            !View {
                mode: Mode::Zoom(999),
                pan: (0.0, 0.0)
            }
            .valid()
        );
        assert!(
            !View {
                mode: Mode::Fit,
                pan: (f32::NAN, 0.0)
            }
            .valid()
        );
    }
}
