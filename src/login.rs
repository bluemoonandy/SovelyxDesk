//! Versioned credential policy. Legacy vault entries always retain their Hello gate.
#[path = "login/preferences.rs"]
pub mod preferences;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use zeroize::Zeroizing;

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protection {
    Quick,
    #[default]
    Hello,
    Never,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Authentication {
    MacAccount,
    VncPassword,
}

impl Authentication {
    pub fn compatible_streaming(
        self,
        mut settings: crate::streaming::StreamSettings,
    ) -> crate::streaming::StreamSettings {
        if self == Self::VncPassword && settings.encoding.is_hevc() {
            settings.encoding = crate::streaming::Encoding::Zrle;
        }
        settings
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub device: Uuid,
    pub host: String,
    pub port: u16,
    pub authentication: Authentication,
    pub username: String,
}

impl Binding {
    pub fn target(&self, protection: Protection) -> Option<String> {
        let mode = match protection {
            Protection::Quick => "quick",
            Protection::Hello => "hello",
            Protection::Never => return None,
        };
        // Length-delimited encoding avoids delimiter ambiguities in account names.
        let encoded = serde_json::to_vec(&(
            self.device,
            self.host.to_ascii_lowercase(),
            self.port,
            self.authentication,
            &self.username,
        ))
        .expect("credential binding contains only JSON primitives");
        let digest = Sha256::digest(encoded);
        Some(format!("MacCanvas/login/v2/{mode}/{digest:x}"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    ManualRequired,
    ConsentRequired,
    Missing,
    Storage,
    ExplicitApprovalRequired,
}

pub type Password = Zeroizing<String>;

pub trait Vault {
    fn read(&self, target: &str) -> Result<Password, Failure>;
    fn write(&self, target: &str, password: &str) -> Result<(), Failure>;
    fn delete(&self, target: &str) -> Result<(), Failure>;
    fn read_legacy_account(&self, _: Uuid) -> Result<(String, Password), Failure> {
        Err(Failure::Storage)
    }
}

pub trait Consent {
    fn verify(&self) -> Result<(), Failure>;
}

pub fn load(
    binding: &Binding,
    protection: Protection,
    vault: &impl Vault,
    consent: &impl Consent,
) -> Result<Password, Failure> {
    let target = binding.target(protection).ok_or(Failure::ManualRequired)?;
    if protection == Protection::Hello {
        consent.verify()?;
    }
    // No fallback to another namespace, authentication method, or old credential.
    vault.read(&target)
}

pub fn load_legacy(
    device: Uuid,
    vault: &impl Vault,
    consent: &impl Consent,
) -> Result<Password, Failure> {
    consent.verify()?;
    vault.read(&format!("MacCanvas/profile/{device}"))
}

pub fn save(
    binding: &Binding,
    protection: Protection,
    explicitly_approved: bool,
    password: &str,
    vault: &impl Vault,
) -> Result<(), Failure> {
    let target = binding.target(protection).ok_or(Failure::ManualRequired)?;
    if !explicitly_approved {
        return Err(Failure::ExplicitApprovalRequired);
    }
    if password.is_empty() || password.len() > 256 || password.contains('\0') {
        return Err(Failure::Storage);
    }
    vault.write(&target, password)
}

pub fn migrate_legacy(
    binding: &Binding,
    protection: Protection,
    explicitly_approved: bool,
    vault: &impl Vault,
    consent: &impl Consent,
) -> Result<(), Failure> {
    if !explicitly_approved || protection == Protection::Never {
        return Err(Failure::ExplicitApprovalRequired);
    }
    consent.verify()?;
    let (username, password) = vault.read_legacy_account(binding.device)?;
    if username != binding.username {
        return Err(Failure::ExplicitApprovalRequired);
    }
    save(binding, protection, true, &password, vault)
    // Preserve the original record so the stable executable can still use it.
}

#[cfg(test)]
#[path = "login/tests.rs"]
mod tests;
