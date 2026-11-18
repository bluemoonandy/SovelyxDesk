use super::*;
use maccanvas::login::{Authentication, Binding, Protection, preferences::Device};

pub(super) fn mode(app: &App) -> Protection {
    match app.get_credential_mode() {
        0 => Protection::Quick,
        1 => Protection::Hello,
        _ => Protection::Never,
    }
}
pub(super) fn binding(app: &App, profile: &Profile) -> Binding {
    Binding {
        device: profile.id,
        host: profile.host.clone(),
        port: profile.port,
        authentication: if profile.authentication == AuthMode::MacLogin {
            Authentication::MacAccount
        } else {
            Authentication::VncPassword
        },
        username: if profile.authentication == AuthMode::MacLogin {
            app.get_username().to_string()
        } else {
            String::new()
        },
    }
}
pub(super) fn entry<'a>(state: &'a State, profile: &Profile) -> Option<&'a Device> {
    state.login.devices.iter().find(|d| {
        d.binding.device == profile.id
            && d.binding.host.eq_ignore_ascii_case(&profile.host)
            && d.binding.port == profile.port
            && (d.binding.authentication == Authentication::MacAccount)
                == (profile.authentication == AuthMode::MacLogin)
    })
}
pub(super) fn select(app: &App, state: &State, profile: &Profile) {
    if let Some(d) = entry(state, profile) {
        app.set_username(d.binding.username.clone().into());
        app.set_credential_mode(match d.protection {
            Protection::Quick => 0,
            Protection::Hello => 1,
            Protection::Never => 2,
        });
        app.set_use_saved(profile.has_saved_credentials && d.protection != Protection::Never);
        app.set_auto_reconnect(d.auto_reconnect);
    } else {
        app.set_credential_mode(1);
        app.set_auto_reconnect(true);
    }
}
pub(super) fn persist(app: &App, state: &mut State, profile: &Profile) -> Result<()> {
    if let Some(error) = &state.login_error {
        return Err(Error::Configuration(error.clone()));
    }
    let mut updated = state.login.clone();
    updated.devices.retain(|d| d.binding.device != profile.id);
    updated.devices.push(Device {
        binding: binding(app, profile),
        protection: mode(app),
        auto_reconnect: app.get_auto_reconnect(),
        streaming: Some(state.settings.streaming),
    });
    updated
        .save(&state.path.with_file_name("connections-v1.json"))
        .map_err(Error::Configuration)?;
    state.login = updated;
    Ok(())
}
pub(super) fn source(app: &App, state: &State, profile: &Profile) -> Result<CredentialSource> {
    let protection = mode(app);
    if app.get_use_saved() {
        if let Some(entry) = entry(state, profile) {
            if entry.protection != protection {
                return Err(Error::Credential(
                    "Enter the password to change credential protection".into(),
                ));
            }
            return Ok(CredentialSource::BoundSaved(
                entry.binding.clone(),
                protection,
            ));
        }
        if protection != Protection::Hello {
            return Err(Error::Credential(
                "Enter your Mac password once to enable quick connection".into(),
            ));
        }
        return Ok(CredentialSource::Saved);
    }
    let credentials = Credentials::new(
        if profile.authentication == AuthMode::MacLogin {
            app.get_username().to_string()
        } else {
            String::new()
        },
        app.get_password().to_string(),
    );
    credentials.validate(profile.authentication)?;
    if protection != Protection::Never && !app.get_credential_approved() {
        return Err(Error::Credential(
            "Confirm saving this password, or choose Do not save".into(),
        ));
    }
    Ok(CredentialSource::BoundManual(
        credentials,
        binding(app, profile),
        protection,
    ))
}
