#![cfg(windows)]
use maccanvas::{
    auth::Credentials,
    credentials::{self, ConsentVerifier, CredentialSource, CredentialStore, WindowsVault},
    error::{Error, Result},
    login::{Authentication, Binding, Protection, Vault},
};
use std::cell::Cell;
use uuid::Uuid;
struct Consent(Cell<u32>);
impl ConsentVerifier for Consent {
    fn verify(&self) -> Result<()> {
        self.0.set(self.0.get() + 1);
        Err(Error::Consent("Synthetic denial".into()))
    }
}
struct Cleanup(Binding);
impl Drop for Cleanup {
    fn drop(&mut self) {
        for mode in [Protection::Quick, Protection::Hello] {
            let _ = Vault::delete(&WindowsVault, &self.0.target(mode).unwrap());
        }
    }
}
#[test]
fn actual_windows_vault_quick_login_is_isolated_and_hello_is_not_bypassed() {
    let binding = Binding {
        device: Uuid::new_v4(),
        host: "192.0.2.18".into(),
        port: 5900,
        authentication: Authentication::MacAccount,
        username: "synthetic-login-fixture".into(),
    };
    let _cleanup = Cleanup(binding.clone());
    let consent = Consent(Cell::new(0));
    let password = Credentials::new(
        binding.username.clone(),
        "synthetic-password-fixture".into(),
    );
    WindowsVault
        .save_bound(&binding, Protection::Quick, &password)
        .unwrap();
    let loaded = credentials::resolve(
        binding.device,
        CredentialSource::BoundSaved(binding.clone(), Protection::Quick),
        &consent,
        &WindowsVault,
    )
    .unwrap();
    assert_eq!(loaded.username(), password.username());
    assert_eq!(loaded.password(), password.password());
    assert_eq!(consent.0.get(), 0);
    let mut other = binding.clone();
    other.port += 1;
    assert!(
        credentials::resolve(
            other.device,
            CredentialSource::BoundSaved(other, Protection::Quick),
            &consent,
            &WindowsVault
        )
        .is_err()
    );
    WindowsVault
        .save_bound(&binding, Protection::Hello, &password)
        .unwrap();
    assert!(
        credentials::resolve(
            binding.device,
            CredentialSource::BoundSaved(binding.clone(), Protection::Hello),
            &consent,
            &WindowsVault
        )
        .is_err()
    );
    assert_eq!(consent.0.get(), 1);
    Vault::delete(&WindowsVault, &binding.target(Protection::Quick).unwrap()).unwrap();
    assert!(
        credentials::resolve(
            binding.device,
            CredentialSource::BoundSaved(binding, Protection::Quick),
            &consent,
            &WindowsVault
        )
        .is_err()
    );
}
