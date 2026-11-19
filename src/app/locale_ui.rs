use super::*;
use crate::{Branding, UiText};
use maccanvas::{brand, i18n};

pub(super) fn initialize(
    app: &App,
    directory: &std::path::Path,
    override_language: Option<i18n::Preference>,
) {
    let branding = app.global::<Branding>();
    branding.set_app_name(brand::APP_NAME.into());
    branding.set_chinese_name(brand::DISPLAY_NAME_ZH.into());
    branding.set_publisher(brand::PUBLISHER.into());
    branding.set_tagline(brand::TAGLINE.into());
    branding.set_chinese_tagline(brand::TAGLINE_ZH.into());
    app.global::<UiText>()
        .on_translate(|source, _| i18n::text(source.as_str()).into());
    let settings = Rc::new(RefCell::new(i18n::Settings::load(
        &directory.join("ui-language-v1.json"),
    )));
    let preference = override_language.unwrap_or(settings.borrow().preference);
    apply(app, preference);
    let weak = app.as_weak();
    app.on_language_changed(move |index| {
        let Some(app) = weak.upgrade() else { return };
        let Some(preference) = i18n::Preference::from_index(index) else {
            return;
        };
        // Only global text/font bindings change; no reconnect, model reset or field setters.
        apply(&app, preference);
        if let Err(error) = settings.borrow_mut().save(preference) {
            notify(&app, format!("Language preference: {error}"));
        }
    });
}
fn apply(app: &App, preference: i18n::Preference) {
    let requested = preference.language(&i18n::system_locale());
    let language = if slint::select_bundled_translation(requested.code()).is_ok() {
        requested
    } else {
        let _ = slint::select_bundled_translation("en");
        i18n::Language::English
    };
    i18n::activate(language);
    let text = app.global::<UiText>();
    text.set_chinese(language == i18n::Language::Chinese);
    text.set_revision(text.get_revision().wrapping_add(1));
    app.set_language_index(preference.index());
}
