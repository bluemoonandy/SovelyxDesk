use std::ptr;

use uuid::Uuid;
use windows::{
    Security::Credentials::UI::{
        UserConsentVerificationResult, UserConsentVerifier, UserConsentVerifierAvailability,
    },
    Win32::{
        Foundation::{ERROR_NOT_FOUND, HWND},
        Security::Credentials::{
            CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree,
            CredReadW, CredWriteW,
        },
        System::WinRT::{
            IUserConsentVerifierInterop, RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize,
        },
        UI::WindowsAndMessaging::IsWindow,
    },
    core::{HSTRING, PCWSTR, PWSTR, factory},
};
use windows_future::IAsyncOperation;
use zeroize::{Zeroize, Zeroizing};

use crate::{
    audit::{self, Level, Scope},
    auth::Credentials,
    error::{Error, Result},
};
use serde_json::json;

pub trait CredentialStore {
    fn load(&self, profile: Uuid) -> Result<Credentials>;
    fn save(&self, profile: Uuid, credentials: &Credentials) -> Result<()>;
    fn delete(&self, profile: Uuid) -> Result<()>;
    fn load_bound(
        &self,
        _: &crate::login::Binding,
        _: crate::login::Protection,
    ) -> Result<Credentials> {
        Err(Error::Credential(
            "Enter the password to configure this login".into(),
        ))
    }
    fn save_bound(
        &self,
        _: &crate::login::Binding,
        _: crate::login::Protection,
        _: &Credentials,
    ) -> Result<()> {
        Err(Error::Credential(
            "This credential store does not support connection policies".into(),
        ))
    }
}

pub trait ConsentVerifier {
    fn verify(&self) -> Result<()>;
}

#[derive(Clone)]
pub enum CredentialSource {
    Manual(Credentials),
    Saved,
    BoundSaved(crate::login::Binding, crate::login::Protection),
    BoundManual(Credentials, crate::login::Binding, crate::login::Protection),
}

impl CredentialSource {
    pub fn validate_for(&self, profile: &crate::profiles::Profile) -> Result<()> {
        if let Self::BoundSaved(binding, _) | Self::BoundManual(_, binding, _) = self {
            let auth = if profile.authentication == crate::profiles::AuthMode::MacLogin {
                crate::login::Authentication::MacAccount
            } else {
                crate::login::Authentication::VncPassword
            };
            if binding.device != profile.id
                || !binding.host.eq_ignore_ascii_case(&profile.host)
                || binding.port != profile.port
                || binding.authentication != auth
            {
                return Err(Error::Credential(
                    "Saved login does not match this device".into(),
                ));
            }
        }
        Ok(())
    }
    pub fn saved(&self) -> Self {
        match self {
            Self::BoundManual(_, binding, mode) | Self::BoundSaved(binding, mode) => {
                Self::BoundSaved(binding.clone(), *mode)
            }
            _ => Self::Saved,
        }
    }
    pub fn save(
        &self,
        profile: Uuid,
        credentials: &Credentials,
        store: &impl CredentialStore,
    ) -> Result<()> {
        match self {
            Self::BoundManual(_, binding, mode) | Self::BoundSaved(binding, mode) => {
                store.save_bound(binding, *mode, credentials)
            }
            _ => store.save(profile, credentials),
        }
    }
}

pub fn resolve(
    profile: Uuid,
    source: CredentialSource,
    consent: &impl ConsentVerifier,
    store: &impl CredentialStore,
) -> Result<Credentials> {
    match source {
        CredentialSource::Saved => {
            consent.verify()?;
            store.load(profile)
        }

        CredentialSource::Manual(credentials)
        | CredentialSource::BoundManual(credentials, _, _) => Ok(credentials),
        CredentialSource::BoundSaved(binding, mode) => {
            if binding.device != profile || mode == crate::login::Protection::Never {
                return Err(Error::Credential(
                    "Enter the password for this device".into(),
                ));
            }
            if mode == crate::login::Protection::Hello {
                consent.verify()?;
            }
            store.load_bound(&binding, mode)
        }
    }
}

pub struct AuditedConsent<'a> {
    pub scope: &'a Scope,
    pub window: isize,
}

impl ConsentVerifier for AuditedConsent<'_> {
    fn verify(&self) -> Result<()> {
        self.scope.critical(
            "security.consent.requested",
            Level::Info,
            json!({"security.mechanism": "windows_hello"}),
        )?;
        let result = WindowsHello {
            window: self.window,
        }
        .verify();
        outcome(self.scope, "security.consent.completed", result)
    }
}

pub struct AuditedVault<'a> {
    pub scope: &'a Scope,
}

fn outcome<T>(scope: &Scope, name: &'static str, result: Result<T>) -> Result<T> {
    match &result {
        Ok(_) => scope.critical(name, Level::Info, json!({"event.outcome": "success"}))?,
        Err(error) => {
            let mut attributes = audit::error_attributes(error);
            attributes["event.outcome"] = json!("failure");
            scope.critical(name, Level::Warn, attributes)?;
        }
    }
    result
}

impl CredentialStore for AuditedVault<'_> {
    fn load_bound(
        &self,
        binding: &crate::login::Binding,
        mode: crate::login::Protection,
    ) -> Result<Credentials> {
        self.scope.critical(
            "credential.read.requested",
            Level::Info,
            json!({"profile.id":binding.device}),
        )?;
        outcome(
            self.scope,
            "credential.read.completed",
            WindowsVault.load_bound(binding, mode),
        )
    }
    fn save_bound(
        &self,
        binding: &crate::login::Binding,
        mode: crate::login::Protection,
        credentials: &Credentials,
    ) -> Result<()> {
        self.scope.critical(
            "credential.save.requested",
            Level::Info,
            json!({"profile.id":binding.device}),
        )?;
        outcome(
            self.scope,
            "credential.save.completed",
            WindowsVault.save_bound(binding, mode, credentials),
        )
    }

    fn load(&self, profile: Uuid) -> Result<Credentials> {
        self.scope.critical(
            "credential.read.requested",
            Level::Info,
            json!({"profile.id": profile}),
        )?;
        outcome(
            self.scope,
            "credential.read.completed",
            WindowsVault.load(profile),
        )
    }

    fn save(&self, profile: Uuid, credentials: &Credentials) -> Result<()> {
        self.scope.critical(
            "credential.save.requested",
            Level::Info,
            json!({"profile.id": profile}),
        )?;
        outcome(
            self.scope,
            "credential.save.completed",
            WindowsVault.save(profile, credentials),
        )
    }

    fn delete(&self, profile: Uuid) -> Result<()> {
        self.scope.critical(
            "credential.delete.requested",
            Level::Info,
            json!({"profile.id": profile}),
        )?;
        outcome(
            self.scope,
            "credential.delete.completed",
            WindowsVault.delete(profile),
        )
    }
}

pub struct WindowsVault;

fn target(profile: Uuid) -> Vec<u16> {
    format!("MacCanvas/profile/{profile}")
        .encode_utf16()
        .chain(Some(0))
        .collect()
}

struct CredentialAllocation(*mut CREDENTIALW);

impl Drop for CredentialAllocation {
    fn drop(&mut self) {
        unsafe {
            let credential = &mut *self.0;
            if !credential.CredentialBlob.is_null() && credential.CredentialBlobSize > 0 {
                std::slice::from_raw_parts_mut(
                    credential.CredentialBlob,
                    credential.CredentialBlobSize as usize,
                )
                .zeroize();
            }
            CredFree(self.0.cast());
        }
    }
}

impl CredentialStore for WindowsVault {
    fn load_bound(
        &self,
        binding: &crate::login::Binding,
        mode: crate::login::Protection,
    ) -> Result<Credentials> {
        let target = binding
            .target(mode)
            .ok_or_else(|| Error::Credential("Enter a password".into()))?;
        let stored = self.load_named(target.encode_utf16().chain(Some(0)).collect())?;
        Ok(Credentials::new(
            binding.username.clone(),
            stored.password().to_owned(),
        ))
    }
    fn save_bound(
        &self,
        binding: &crate::login::Binding,
        mode: crate::login::Protection,
        credentials: &Credentials,
    ) -> Result<()> {
        if credentials.username() != binding.username {
            return Err(Error::Credential(
                "Login account does not match the selected device".into(),
            ));
        }
        let target = binding
            .target(mode)
            .ok_or_else(|| Error::Credential("Password saving is disabled".into()))?;
        self.save_named(target.encode_utf16().chain(Some(0)).collect(), credentials)
    }

    fn load(&self, profile: Uuid) -> Result<Credentials> {
        self.load_named(target(profile))
    }
    fn save(&self, profile: Uuid, credentials: &Credentials) -> Result<()> {
        self.save_named(target(profile), credentials)
    }
    fn delete(&self, profile: Uuid) -> Result<()> {
        self.delete_named(target(profile))
    }
}

impl crate::login::Vault for WindowsVault {
    fn read_legacy_account(
        &self,
        profile: Uuid,
    ) -> std::result::Result<(String, Zeroizing<String>), crate::login::Failure> {
        let c = CredentialStore::load(self, profile).map_err(|_| crate::login::Failure::Storage)?;
        Ok((
            c.username().to_owned(),
            Zeroizing::new(c.password().to_owned()),
        ))
    }

    fn read(&self, target: &str) -> std::result::Result<Zeroizing<String>, crate::login::Failure> {
        let credentials = self
            .load_named(target.encode_utf16().chain(Some(0)).collect())
            .map_err(|_| crate::login::Failure::Storage)?;
        Ok(Zeroizing::new(credentials.password().to_owned()))
    }
    fn write(
        &self,
        target: &str,
        password: &str,
    ) -> std::result::Result<(), crate::login::Failure> {
        self.save_named(
            target.encode_utf16().chain(Some(0)).collect(),
            &Credentials::new(String::new(), password.to_owned()),
        )
        .map_err(|_| crate::login::Failure::Storage)
    }
    fn delete(&self, target: &str) -> std::result::Result<(), crate::login::Failure> {
        self.delete_named(target.encode_utf16().chain(Some(0)).collect())
            .map_err(|_| crate::login::Failure::Storage)
    }
}

impl crate::login::Consent for AuditedConsent<'_> {
    fn verify(&self) -> std::result::Result<(), crate::login::Failure> {
        ConsentVerifier::verify(self).map_err(|_| crate::login::Failure::ConsentRequired)
    }
}

impl WindowsVault {
    fn load_named(&self, name: Vec<u16>) -> Result<Credentials> {
        let mut pointer = ptr::null_mut();
        unsafe { CredReadW(PCWSTR(name.as_ptr()), CRED_TYPE_GENERIC, None, &mut pointer) }
            .map_err(|error| {
                if error.code() == ERROR_NOT_FOUND.to_hresult() {
                    Error::Credential("Saved credentials were removed. Enter them manually.".into())
                } else {
                    Error::Credential(error.to_string())
                }
            })?;
        if pointer.is_null() {
            return Err(Error::Credential(
                "Windows returned an invalid credential".into(),
            ));
        }
        let allocation = CredentialAllocation(pointer);
        let raw = unsafe { &*allocation.0 };
        if raw.CredentialBlobSize == 0
            || raw.CredentialBlobSize > 256
            || raw.CredentialBlob.is_null()
        {
            return Err(Error::Credential(
                "Saved password has an invalid size".into(),
            ));
        }
        let mut password = Zeroizing::new(
            unsafe {
                std::slice::from_raw_parts(raw.CredentialBlob, raw.CredentialBlobSize as usize)
            }
            .to_vec(),
        );
        let password_string = std::str::from_utf8(&password)
            .map_err(|_| Error::Credential("Saved password is not UTF-8".into()))?
            .to_owned();
        password.zeroize();
        let username = if raw.UserName.is_null() {
            String::new()
        } else {
            unsafe { raw.UserName.to_string() }
                .map_err(|_| Error::Credential("Saved username is invalid".into()))?
        };
        Ok(Credentials::new(username, password_string))
    }

    fn save_named(&self, mut name: Vec<u16>, credentials: &Credentials) -> Result<()> {
        if credentials.password().is_empty() || credentials.password().len() > 256 {
            return Err(Error::Credential("Password has an invalid size".into()));
        }
        let mut username: Vec<u16> = credentials
            .username()
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut blob = Zeroizing::new(credentials.password().as_bytes().to_vec());
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(name.as_mut_ptr()),
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: PWSTR(username.as_mut_ptr()),
            ..CREDENTIALW::default()
        };
        let result = unsafe { CredWriteW(&credential, 0) }
            .map_err(|error| Error::Credential(error.to_string()));
        username.zeroize();
        result
    }

    fn delete_named(&self, name: Vec<u16>) -> Result<()> {
        match unsafe { CredDeleteW(PCWSTR(name.as_ptr()), CRED_TYPE_GENERIC, None) } {
            Ok(()) => Ok(()),
            Err(error) if error.code() == ERROR_NOT_FOUND.to_hresult() => Ok(()),
            Err(error) => Err(Error::Credential(error.to_string())),
        }
    }
}

pub struct WindowsHello {
    pub window: isize,
}

struct Runtime;

impl Runtime {
    fn initialize() -> Result<Self> {
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }
            .map_err(|error| Error::Consent(error.to_string()))?;
        Ok(Self)
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        unsafe { RoUninitialize() };
    }
}

impl ConsentVerifier for WindowsHello {
    fn verify(&self) -> Result<()> {
        let _runtime = Runtime::initialize()?;
        let window = HWND(self.window as *mut _);
        if self.window == 0 || !unsafe { IsWindow(Some(window)) }.as_bool() {
            return Err(Error::Consent(
                "The connection window is no longer available".into(),
            ));
        }
        let availability = UserConsentVerifier::CheckAvailabilityAsync()
            .and_then(|operation| operation.join())
            .map_err(|error| Error::Consent(error.to_string()))?;
        if availability != UserConsentVerifierAvailability::Available {
            return Err(Error::Consent("Set up Windows Hello in Windows Settings, or enter your Mac credentials manually instead of using the saved password.".into()));
        }
        let verifier = factory::<UserConsentVerifier, IUserConsentVerifierInterop>()
            .map_err(|error| Error::Consent(error.to_string()))?;
        let operation: IAsyncOperation<UserConsentVerificationResult> = unsafe {
            verifier.RequestVerificationForWindowAsync(
                window,
                &HSTRING::from("Verify to connect with MacCanvas credentials"),
            )
        }
        .map_err(|error| Error::Consent(error.to_string()))?;
        let result = operation
            .join()
            .map_err(|error| Error::Consent(error.to_string()))?;
        if result != UserConsentVerificationResult::Verified {
            return Err(Error::Consent("Verification was not approved. No saved password was read and no connection was opened.".into()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Store<'a>(&'a RefCell<Vec<&'static str>>);
    impl CredentialStore for Store<'_> {
        fn load_bound(
            &self,
            binding: &crate::login::Binding,
            _: crate::login::Protection,
        ) -> Result<Credentials> {
            self.0.borrow_mut().push("read_bound");
            Ok(Credentials::new(binding.username.clone(), "fixture".into()))
        }

        fn load(&self, _: Uuid) -> Result<Credentials> {
            self.0.borrow_mut().push("read");
            Ok(Credentials::new("fixture".into(), "fixture".into()))
        }
        fn save(&self, _: Uuid, _: &Credentials) -> Result<()> {
            Ok(())
        }
        fn delete(&self, _: Uuid) -> Result<()> {
            Ok(())
        }
    }
    struct Consent<'a>(&'a RefCell<Vec<&'static str>>, bool);
    impl ConsentVerifier for Consent<'_> {
        fn verify(&self) -> Result<()> {
            self.0.borrow_mut().push("hello");
            if self.1 {
                Ok(())
            } else {
                Err(Error::Consent("cancelled fixture".into()))
            }
        }
    }

    #[test]
    fn bound_sources_enforce_policy_before_vault_access() {
        use crate::login::{Authentication, Binding, Protection};
        let calls = RefCell::new(Vec::new());
        let id = Uuid::new_v4();
        let binding = Binding {
            device: id,
            host: "mac.local".into(),
            port: 5900,
            authentication: Authentication::MacAccount,
            username: "fixture".into(),
        };
        resolve(
            id,
            CredentialSource::BoundSaved(binding.clone(), Protection::Quick),
            &Consent(&calls, false),
            &Store(&calls),
        )
        .unwrap();
        assert_eq!(*calls.borrow(), ["read_bound"]);
        calls.borrow_mut().clear();
        assert!(
            resolve(
                id,
                CredentialSource::BoundSaved(binding.clone(), Protection::Hello),
                &Consent(&calls, false),
                &Store(&calls)
            )
            .is_err()
        );
        assert_eq!(*calls.borrow(), ["hello"]);
        calls.borrow_mut().clear();
        assert!(
            resolve(
                Uuid::new_v4(),
                CredentialSource::BoundSaved(binding.clone(), Protection::Quick),
                &Consent(&calls, true),
                &Store(&calls)
            )
            .is_err()
        );
        assert!(
            resolve(
                id,
                CredentialSource::BoundSaved(binding, Protection::Never),
                &Consent(&calls, true),
                &Store(&calls)
            )
            .is_err()
        );
        assert!(calls.borrow().is_empty());
    }

    #[test]
    fn saved_secrets_are_read_only_after_approval() {
        let calls = RefCell::new(Vec::new());
        resolve(
            Uuid::new_v4(),
            CredentialSource::Saved,
            &Consent(&calls, true),
            &Store(&calls),
        )
        .unwrap();
        assert_eq!(*calls.borrow(), ["hello", "read"]);
    }

    #[test]
    fn cancelled_hello_never_reads_the_vault() {
        let calls = RefCell::new(Vec::new());
        assert!(
            resolve(
                Uuid::new_v4(),
                CredentialSource::Saved,
                &Consent(&calls, false),
                &Store(&calls)
            )
            .is_err()
        );
        assert_eq!(*calls.borrow(), ["hello"]);
    }

    #[test]
    fn manual_credentials_do_not_read_the_vault() {
        let calls = RefCell::new(Vec::new());
        resolve(
            Uuid::new_v4(),
            CredentialSource::Manual(Credentials::new("fixture".into(), "fixture".into())),
            &Consent(&calls, false),
            &Store(&calls),
        )
        .unwrap();
        assert!(calls.borrow().is_empty());
    }

    #[test]
    fn every_saved_login_requires_a_fresh_hello_approval() {
        let calls = RefCell::new(Vec::new());
        let profile = Uuid::new_v4();
        resolve(
            profile,
            CredentialSource::Saved,
            &Consent(&calls, true),
            &Store(&calls),
        )
        .unwrap();
        assert!(
            resolve(
                profile,
                CredentialSource::Saved,
                &Consent(&calls, false),
                &Store(&calls)
            )
            .is_err()
        );
        assert_eq!(*calls.borrow(), ["hello", "read", "hello"]);
    }

    #[test]
    fn cloning_a_reconnect_source_cannot_cache_saved_secrets_or_hello_consent() {
        let calls = RefCell::new(Vec::new());
        let source = CredentialSource::Saved;
        let profile = Uuid::new_v4();
        resolve(
            profile,
            source.clone(),
            &Consent(&calls, true),
            &Store(&calls),
        )
        .unwrap();
        assert!(resolve(profile, source, &Consent(&calls, false), &Store(&calls)).is_err());
        assert_eq!(*calls.borrow(), ["hello", "read", "hello"]);
    }

    #[test]
    fn unsaved_reconnect_credentials_never_use_the_vault() {
        let calls = RefCell::new(Vec::new());
        let source = CredentialSource::Manual(Credentials::new("fixture".into(), "fixture".into()));
        for reconnect in [source.clone(), source] {
            let credentials = resolve(
                Uuid::new_v4(),
                reconnect,
                &Consent(&calls, false),
                &Store(&calls),
            )
            .unwrap();
            assert_eq!(credentials.username(), "fixture");
            assert_eq!(credentials.password(), "fixture");
        }
        assert!(calls.borrow().is_empty());
    }

    #[test]
    fn windows_vault_round_trip_uses_only_a_disposable_test_target() {
        let profile = Uuid::new_v4();
        let credentials = Credentials::new("fixture-user".into(), "fixture-password".into());
        WindowsVault.save(profile, &credentials).unwrap();
        let restored = WindowsVault.load(profile);
        WindowsVault.delete(profile).unwrap();
        let restored = restored.unwrap();
        assert_eq!(restored.username(), credentials.username());
        assert_eq!(restored.password(), credentials.password());
    }
}
