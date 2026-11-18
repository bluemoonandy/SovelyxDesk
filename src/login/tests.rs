use super::*;
use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Default)]
struct Store {
    records: RefCell<HashMap<String, String>>,
    calls: RefCell<Vec<&'static str>>,
}
impl Vault for Store {
    fn read(&self, target: &str) -> Result<Password, Failure> {
        self.calls.borrow_mut().push("read");
        self.records
            .borrow()
            .get(target)
            .cloned()
            .map(Zeroizing::new)
            .ok_or(Failure::Missing)
    }
    fn write(&self, target: &str, password: &str) -> Result<(), Failure> {
        self.calls.borrow_mut().push("write");
        self.records
            .borrow_mut()
            .insert(target.into(), password.into());
        Ok(())
    }
    fn delete(&self, target: &str) -> Result<(), Failure> {
        self.records.borrow_mut().remove(target);
        Ok(())
    }
}
struct Approval(bool, RefCell<u32>);
impl Consent for Approval {
    fn verify(&self) -> Result<(), Failure> {
        *self.1.borrow_mut() += 1;
        if self.0 {
            Ok(())
        } else {
            Err(Failure::ConsentRequired)
        }
    }
}
fn binding() -> Binding {
    Binding {
        device: Uuid::nil(),
        host: "mac.local".into(),
        port: 5900,
        authentication: Authentication::MacAccount,
        username: "andy".into(),
    }
}

#[test]
fn quick_reopen_does_not_invoke_hello() {
    let b = binding();
    let store = Store::default();
    let consent = Approval(false, RefCell::new(0));
    save(&b, Protection::Quick, true, "fixture", &store).unwrap();
    assert_eq!(
        &*load(&b, Protection::Quick, &store, &consent).unwrap(),
        "fixture"
    );
    assert_eq!(*consent.1.borrow(), 0);
}
#[test]
fn hello_denial_never_reads_saved_secret() {
    let store = Store::default();
    let consent = Approval(false, RefCell::new(0));
    assert_eq!(
        load(&binding(), Protection::Hello, &store, &consent),
        Err(Failure::ConsentRequired)
    );
    assert!(store.calls.borrow().is_empty());
}
#[test]
fn never_mode_never_touches_vault_or_consent() {
    let store = Store::default();
    let consent = Approval(false, RefCell::new(0));
    assert_eq!(
        load(&binding(), Protection::Never, &store, &consent),
        Err(Failure::ManualRequired)
    );
    assert_eq!(
        save(&binding(), Protection::Never, true, "fixture", &store),
        Err(Failure::ManualRequired)
    );
    assert!(store.calls.borrow().is_empty());
    assert_eq!(*consent.1.borrow(), 0);
}
#[test]
fn changing_config_flag_cannot_downgrade_existing_hello_record() {
    let b = binding();
    let store = Store::default();
    let consent = Approval(false, RefCell::new(0));
    save(&b, Protection::Hello, true, "fixture", &store).unwrap();
    assert_eq!(
        load(&b, Protection::Quick, &store, &consent),
        Err(Failure::Missing)
    );
}
#[test]
fn legacy_migration_requires_explicit_approval_and_hello() {
    let b = binding();
    let store = Store::default();
    let denied = Approval(false, RefCell::new(0));
    store
        .records
        .borrow_mut()
        .insert(format!("MacCanvas/profile/{}", b.device), "fixture".into());
    assert_eq!(
        migrate_legacy(&b, Protection::Quick, false, &store, &denied),
        Err(Failure::ExplicitApprovalRequired)
    );
    assert_eq!(*denied.1.borrow(), 0);
    assert_eq!(
        migrate_legacy(&b, Protection::Quick, true, &store, &denied),
        Err(Failure::ConsentRequired)
    );
    assert!(store.calls.borrow().is_empty());
    let allowed = Approval(true, RefCell::new(0));
    migrate_legacy(&b, Protection::Quick, true, &store, &allowed).unwrap();
    assert_eq!(store.records.borrow().len(), 2);
    assert_eq!(
        &*load(&b, Protection::Quick, &store, &denied).unwrap(),
        "fixture"
    );
}
#[test]
fn all_endpoint_and_account_fields_isolate_credentials() {
    let b = binding();
    let key = b.target(Protection::Quick);
    let mut changes = vec![b.clone(); 5];
    changes[0].device = Uuid::new_v4();
    changes[1].host = "other.local".into();
    changes[2].port = 5901;
    changes[3].authentication = Authentication::VncPassword;
    changes[4].username = "other".into();
    for other in changes {
        assert_ne!(key, other.target(Protection::Quick));
    }
    let mut alias = b.clone();
    alias.host = "MAC.LOCAL".into();
    assert_eq!(key, alias.target(Protection::Quick));
}
#[test]
fn hello_is_checked_for_every_load_and_failed_read_is_not_retried() {
    let store = Store::default();
    let consent = Approval(true, RefCell::new(0));
    for _ in 0..2 {
        assert_eq!(
            load(&binding(), Protection::Hello, &store, &consent),
            Err(Failure::Missing)
        );
    }
    assert_eq!(*consent.1.borrow(), 2);
    assert_eq!(store.calls.borrow().len(), 2);
}
#[test]
fn no_password_or_username_is_exposed_in_vault_target() {
    let key = binding().target(Protection::Quick).unwrap();
    assert!(!key.contains("andy"));
    assert!(!key.contains("mac.local"));
    assert_eq!(Protection::default(), Protection::Hello);
}
#[test]
fn quick_save_requires_user_authorization() {
    let store = Store::default();
    assert_eq!(
        save(&binding(), Protection::Quick, false, "fixture", &store),
        Err(Failure::ExplicitApprovalRequired)
    );
    assert!(store.calls.borrow().is_empty());
}
