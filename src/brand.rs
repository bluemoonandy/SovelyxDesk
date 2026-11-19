//! Product metadata is compiled from brand/brand.json. Legacy storage identities stay stable.
pub const APP_NAME: &str = env!("SOVELYX_APP_NAME");
pub const DISPLAY_NAME_ZH: &str = env!("SOVELYX_DISPLAY_NAME_ZH_CN");
pub const PUBLISHER: &str = env!("SOVELYX_PUBLISHER");
pub const TAGLINE: &str = env!("SOVELYX_TAGLINE");
pub const TAGLINE_ZH: &str = env!("SOVELYX_TAGLINE_ZH_CN");
pub const DESCRIPTION: &str = env!("SOVELYX_DESCRIPTION");
pub const EXECUTABLE: &str = env!("SOVELYX_EXECUTABLE");
pub const RELEASE_NAME: &str = env!("SOVELYX_RELEASE_NAME");
pub const AUDIO_ASSET: &str = env!("SOVELYX_AUDIO_ASSET");
pub const SHORTCUT: &str = env!("SOVELYX_SHORTCUT");
pub const INSTALL_RECEIPT: &str = env!("SOVELYX_INSTALL_RECEIPT");
pub const HOMEPAGE: &str = env!("SOVELYX_HOMEPAGE");
pub const REPOSITORY: &str = env!("SOVELYX_REPOSITORY");
pub const REPOSITORY_SLUG: &str = env!("SOVELYX_REPOSITORY_SLUG");
pub const UPDATE_SOURCE: &str = env!("SOVELYX_UPDATE_SOURCE");
pub const PRODUCT_VERSION: &str = crate::VERSION;
pub const BUILD_COMMIT: &str = crate::REVISION;
pub const BUILD_DATE: &str = crate::BUILD_UTC;
pub const UPDATES_READY: bool = matches!(env!("SOVELYX_UPDATES_READY").as_bytes(), b"true");
pub const UPDATES_UNAVAILABLE: &str = "Sovelyx Desk updates are not published yet. Automatic downloads and installation are disabled.";

pub fn source_url() -> String {
    let revision = crate::REVISION;
    if revision.len() == 40 && revision.bytes().all(|b| b.is_ascii_hexdigit()) {
        format!("{REPOSITORY}/tree/{revision}")
    } else {
        REPOSITORY.into()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn brand_is_confirmed_without_changing_legacy_storage() {
        let metadata: serde_json::Value =
            serde_json::from_str(include_str!("../brand/brand.json")).unwrap();
        assert_eq!(metadata["confirmed"], true);
        assert_eq!(APP_NAME, "Sovelyx Desk");
        assert_eq!(EXECUTABLE, "SovelyxDesk.exe");
        assert_eq!(metadata["legacy_data_directory"], "MacCanvas");
        assert_eq!(
            metadata["legacy_credential_prefixes"][0],
            "MacCanvas/profile/"
        );
        assert_eq!(metadata["updates_ready"], UPDATES_READY);
        assert_eq!(metadata["repository"], REPOSITORY);
    }
}
