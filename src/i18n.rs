//! Presentation-only localization. Protocol values, credentials and editable input stay untouched.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preference {
    #[default]
    System,
    Chinese,
    English,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    English,
    Chinese,
}
impl Language {
    pub fn code(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Chinese => "zh_CN",
        }
    }
}
impl Preference {
    pub fn from_index(index: i32) -> Option<Self> {
        match index {
            0 => Some(Self::System),
            1 => Some(Self::Chinese),
            2 => Some(Self::English),
            _ => None,
        }
    }
    pub fn index(self) -> i32 {
        match self {
            Self::System => 0,
            Self::Chinese => 1,
            Self::English => 2,
        }
    }
    pub fn language(self, system: &str) -> Language {
        match self {
            Self::Chinese => Language::Chinese,
            Self::English => Language::English,
            Self::System => language_for_locale(system),
        }
    }
}
pub fn language_for_locale(locale: &str) -> Language {
    let locale = locale
        .split(['.', '@'])
        .next()
        .unwrap_or("")
        .replace('_', "-")
        .to_ascii_lowercase();
    if locale == "zh"
        || locale == "zh-cn"
        || locale == "zh-sg"
        || locale == "zh-hans"
        || locale.starts_with("zh-hans-")
    {
        Language::Chinese
    } else {
        Language::English
    }
}
pub fn system_locale() -> String {
    #[cfg(windows)]
    {
        let mut buffer = [0u16; 85];
        let length =
            unsafe { windows::Win32::Globalization::GetUserDefaultLocaleName(&mut buffer) };
        if length > 1 {
            return String::from_utf16_lossy(&buffer[..length as usize - 1]);
        }
    }
    std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_else(|_| "en".into())
}
// The consent worker uses the same application language as the Slint UI thread.
static CHINESE: AtomicBool = AtomicBool::new(false);
pub fn activate(language: Language) {
    CHINESE.store(language == Language::Chinese, Ordering::Release);
}
pub fn text(source: &str) -> String {
    translate(
        if CHINESE.load(Ordering::Acquire) {
            Language::Chinese
        } else {
            Language::English
        },
        source,
    )
}
fn catalog() -> &'static BTreeMap<String, String> {
    static CATALOG: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("../translations/zh_CN.json")).unwrap_or_default()
    })
}
// Named interpolation stays in the catalogue. Values such as paths and device names
// are copied byte-for-byte; only explicitly named error/reason fields recurse.
fn parts(template: &str) -> Option<(Vec<&str>, Vec<&str>)> {
    let mut literals = Vec::new();
    let mut names = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        let end = rest[start + 1..].find('}')? + start + 1;
        if rest.get(start + 1..start + 2) == Some("{") {
            return None;
        }
        literals.push(&rest[..start]);
        names.push(&rest[start + 1..end]);
        rest = &rest[end + 1..];
    }
    literals.push(rest);
    (!names.is_empty()).then_some((literals, names))
}
fn render_match(source: &str, english: &str, chinese: &str, depth: u8) -> Option<String> {
    let (literals, names) = parts(english)?;
    if literals.iter().map(|s| s.len()).sum::<usize>() < 4 {
        return None;
    }
    let mut rest = source.strip_prefix(literals[0])?;
    let mut values = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let next = literals[index + 1];
        let (value, tail) = if index + 1 == names.len() {
            (rest.strip_suffix(next)?, "")
        } else {
            if next.is_empty() {
                return None;
            }
            let at = rest.find(next)?;
            (&rest[..at], &rest[at + next.len()..])
        };
        values.push((*name, value));
        rest = tail;
    }
    let mut output = String::new();
    let mut rest = chinese;
    while let Some(start) = rest.find('{') {
        let end = rest[start + 1..].find('}')? + start + 1;
        output.push_str(&rest[..start]);
        let name = &rest[start + 1..end];
        let value = values.iter().find(|(key, _)| *key == name)?.1;
        if depth < 5 && matches!(name, "error" | "reason" | "message" | "detail") {
            output.push_str(&translate_inner(value, depth + 1));
        } else {
            output.push_str(value);
        }
        rest = &rest[end + 1..];
    }
    output.push_str(rest);
    Some(output)
}
fn translate_inner(source: &str, depth: u8) -> String {
    if let Some(value) = catalog().get(source).filter(|s| !s.is_empty()) {
        return value.clone();
    }
    if source.len() <= 16 * 1024 {
        // Prefer more specific patterns so a generic error wrapper never hides a full message.
        static PATTERNS: OnceLock<Vec<(&'static str, &'static str)>> = OnceLock::new();
        let patterns = PATTERNS.get_or_init(|| {
            let mut values: Vec<_> = catalog()
                .iter()
                .filter(|(s, _)| s.contains('{'))
                .map(|(a, b)| (a.as_str(), b.as_str()))
                .collect();
            values.sort_by_key(|(s, _)| std::cmp::Reverse(s.len()));
            values
        });
        for (english, chinese) in patterns {
            if let Some(value) = render_match(source, english, chinese, depth) {
                return value;
            }
        }
    }
    source.into()
}
pub fn translate(language: Language, source: &str) -> String {
    if language == Language::Chinese {
        translate_inner(source, 0)
    } else {
        source.into()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    version: u8,
    language: Preference,
}
pub struct Settings {
    pub preference: Preference,
    path: PathBuf,
    load_error: bool,
}
impl Settings {
    pub fn load(path: &Path) -> Self {
        let result = std::fs::read(path);
        let (preference, load_error) = match result {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                (Preference::System, false)
            }
            Ok(bytes) if bytes.len() <= 4096 => match serde_json::from_slice::<Saved>(&bytes) {
                Ok(saved) if saved.version == 1 => (saved.language, false),
                _ => (Preference::System, true),
            },
            _ => (Preference::System, true),
        };
        Self {
            preference,
            path: path.into(),
            load_error,
        }
    }
    pub fn save(&mut self, preference: Preference) -> std::io::Result<()> {
        if self.load_error {
            return Err(std::io::Error::other(
                "Original language preferences were preserved because the file is invalid",
            ));
        }
        let parent = self
            .path
            .parent()
            .ok_or_else(|| std::io::Error::other("Missing language preferences directory"))?;
        std::fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer(
            &mut file,
            &Saved {
                version: 1,
                language: preference,
            },
        )?;
        file.flush()?;
        file.as_file().sync_all()?;
        file.persist(&self.path).map_err(|e| e.error)?;
        self.preference = preference;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn locales_and_manual_overrides_are_predictable() {
        for s in ["zh-CN", "zh_CN.UTF-8", "zh-SG", "zh-Hans", "zh-Hans-CN"] {
            assert_eq!(language_for_locale(s), Language::Chinese);
        }
        for s in [
            "en-US", "en-GB", "de-DE", "zh-TW", "zh-Hant", "zh-HK", "", "fr",
        ] {
            assert_eq!(language_for_locale(s), Language::English);
        }
        assert_eq!(Preference::English.language("zh-CN"), Language::English);
        assert_eq!(Preference::Chinese.language("en-US"), Language::Chinese);
        for i in 0..=2 {
            assert_eq!(Preference::from_index(i).unwrap().index(), i);
        }
    }
    #[test]
    fn translation_falls_back_and_preserves_interpolated_user_data() {
        assert_eq!(translate(Language::English, "Connect"), "Connect");
        assert_eq!(translate(Language::Chinese, "Connect"), "连接");
        assert_eq!(
            translate(Language::Chinese, "unknown message"),
            "unknown message"
        );
        assert_eq!(
            translate(Language::Chinese, "Connected to host (Connect)"),
            "已连接到 host（Connect）"
        );
        assert_eq!(
            translate(Language::Chinese, "File upload: invalid path"),
            "文件上传：invalid path"
        );
    }
    #[test]
    fn backend_errors_translate_without_rewriting_file_paths_or_device_names() {
        let path = r"C:\Users\Connect\原稿.txt";
        let message =
            format!("Cannot save the new file without overwriting an existing file: {path}");
        let translated = translate(Language::Chinese, &message);
        assert!(translated.starts_with("无法在不覆盖已有文件"));
        assert!(translated.ends_with(path));
        assert_eq!(translate(Language::English, &message), message);
        assert_eq!(
            translate(Language::Chinese, "Connected to host (MacCanvas)"),
            "已连接到 host（MacCanvas）"
        );
        assert!(translate(Language::Chinese, "Security audit: Audit queue exhausted; restart the application before making another connection").contains("审计队列已满"));
    }
    #[test]
    fn consent_workers_share_the_selected_language() {
        activate(Language::Chinese);
        let prompt = std::thread::spawn(|| text("Verify to connect with Sovelyx Desk credentials"))
            .join()
            .unwrap();
        activate(Language::English);
        assert_eq!(prompt, "验证以使用 Sovelyx Desk 保存的凭据连接");
    }
    #[test]
    fn language_preferences_are_separate_and_corruption_is_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ui-language-v1.json");
        std::fs::write(dir.path().join("connections-v1.json"), b"keep original").unwrap();
        let mut settings = Settings::load(&path);
        settings.save(Preference::Chinese).unwrap();
        assert_eq!(Settings::load(&path).preference, Preference::Chinese);
        assert_eq!(
            std::fs::read(dir.path().join("connections-v1.json")).unwrap(),
            b"keep original"
        );
        std::fs::write(&path, b"broken").unwrap();
        let mut broken = Settings::load(&path);
        assert_eq!(broken.preference, Preference::System);
        assert!(broken.save(Preference::English).is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"broken");
    }
}
