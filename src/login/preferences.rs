//! Sidecar data only; the strict stable settings schema remains untouched.
use super::{Binding, Protection};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Device {
    pub binding: Binding,
    pub protection: Protection,
    #[serde(default = "enabled")]
    pub auto_reconnect: bool,
}
fn enabled() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    pub version: u32,
    pub devices: Vec<Device>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: 1,
            devices: Vec::new(),
        }
    }
}
impl Preferences {
    pub fn parse(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() > 1024 * 1024 {
            return Err("Connection preferences exceed the size limit");
        }
        let parsed: Self = serde_json::from_slice(bytes).map_err(
            |_| "Connection preferences are invalid; original device settings are unchanged",
        )?;
        if parsed.version != 1 || parsed.devices.len() > 128 {
            return Err("Unsupported connection preferences");
        }
        let mut ids = HashSet::new();
        for device in &parsed.devices {
            let binding = &device.binding;
            if !ids.insert(binding.device)
                || binding.port == 0
                || binding.host.is_empty()
                || binding.host.len() > 253
                || binding
                    .host
                    .chars()
                    .any(|c| c.is_whitespace() || c.is_control() || "/\\@".contains(c))
                || binding.username.len() > 256
                || binding.username.chars().any(char::is_control)
            {
                return Err("Invalid saved connection preferences");
            }
        }
        Ok(parsed)
    }
}

impl Preferences {
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        use std::io::Read;
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(_) => return Err("Connection preferences could not be read".into()),
        };
        let mut bytes = Vec::new();
        file.take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "Connection preferences could not be read")?;
        Self::parse(&bytes).map_err(str::to_owned)
    }
    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        use std::io::Write;
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|_| "Connection preferences could not be encoded")?;
        Self::parse(&bytes).map_err(str::to_owned)?;
        let parent = path.parent().ok_or("Missing settings directory")?;
        std::fs::create_dir_all(parent).map_err(|_| "Settings directory is unavailable")?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)
            .map_err(|_| "Cannot create connection preferences")?;
        temporary
            .write_all(&bytes)
            .map_err(|_| "Cannot write connection preferences")?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|_| "Cannot flush connection preferences")?;
        temporary
            .persist(path)
            .map_err(|_| "Cannot replace connection preferences")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_round_trip_and_corruption_preserve_legacy_device_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connections-v1.json");
        let legacy = dir.path().join("settings.json");
        std::fs::write(&legacy, b"legacy fixture").unwrap();
        assert!(Preferences::load(&path).unwrap().devices.is_empty());
        let prefs = Preferences::default();
        prefs.save(&path).unwrap();
        assert!(Preferences::load(&path).unwrap().devices.is_empty());
        std::fs::write(&path, b"damaged").unwrap();
        assert!(Preferences::load(&path).is_err());
        assert_eq!(std::fs::read(&legacy).unwrap(), b"legacy fixture");
        assert_eq!(std::fs::read(&path).unwrap(), b"damaged");
    }
    #[test]
    fn duplicate_devices_and_oversize_input_are_rejected() {
        let b = super::super::Binding {
            device: uuid::Uuid::nil(),
            host: "mac.local".into(),
            port: 5900,
            authentication: super::super::Authentication::MacAccount,
            username: "fixture".into(),
        };
        let d = Device {
            binding: b,
            protection: Protection::Quick,
            auto_reconnect: true,
        };
        assert!(
            Preferences::parse(
                &serde_json::to_vec(&Preferences {
                    version: 1,
                    devices: vec![d.clone(), d]
                })
                .unwrap()
            )
            .is_err()
        );
        assert!(Preferences::parse(&vec![b' '; 1024 * 1024 + 1]).is_err());
    }
    #[test]
    fn unknown_secret_fields_and_future_versions_are_rejected() {
        assert!(Preferences::parse(br#"{"version":1,"devices":[],"password":"fixture"}"#).is_err());
        assert!(Preferences::parse(br#"{"version":2,"devices":[]}"#).is_err());
        assert!(Preferences::parse(b"corrupt").is_err());
    }
    #[test]
    fn empty_defaults_round_trip_without_affecting_legacy_settings() {
        let bytes = serde_json::to_vec(&Preferences::default()).unwrap();
        assert_eq!(Preferences::parse(&bytes).unwrap().devices.len(), 0);
        assert!(!String::from_utf8(bytes).unwrap().contains("password"));
    }
}
