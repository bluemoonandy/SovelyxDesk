//! Passive HTML/RTF clipboard data. Never rendered, executed, or logged by this module.
use crate::error::{Error, Result};
use zeroize::Zeroizing;
pub const MAX_RICH: usize = 1024 * 1024;
#[derive(Default)]
pub struct RichText {
    pub plain: Option<Zeroizing<String>>,
    pub html: Option<Zeroizing<String>>,
    pub rtf: Option<Zeroizing<Vec<u8>>>,
}
fn invalid(reason: &str) -> Error {
    Error::Clipboard(format!("Rich clipboard: {reason}"))
}
pub fn html(bytes: &[u8]) -> Result<Zeroizing<String>> {
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    if bytes.is_empty() || bytes.len() > MAX_RICH || bytes.contains(&0) {
        return Err(invalid("invalid HTML size or encoding"));
    }
    Ok(Zeroizing::new(
        std::str::from_utf8(bytes)
            .map_err(|_| invalid("HTML is not UTF-8"))?
            .to_owned(),
    ))
}
/// CF_HTML mandates UTF-8; public.html also needs an explicit encoding for Cocoa.
pub fn mac_html(source: &str) -> Result<Zeroizing<Vec<u8>>> {
    let normalized = html(source.as_bytes())?;
    let mut bytes = Zeroizing::new(Vec::with_capacity(normalized.len() + 3));
    bytes.extend_from_slice(&[0xef, 0xbb, 0xbf]);
    bytes.extend_from_slice(normalized.as_bytes());
    Ok(bytes)
}
pub fn validate_rtf(bytes: &[u8]) -> Result<()> {
    if bytes.len() > MAX_RICH
        || !bytes.starts_with(b"{\\rtf")
        || !bytes.ends_with(b"}")
        || bytes.contains(&0)
    {
        return Err(invalid("unsupported RTF encoding or size"));
    }
    Ok(())
}
impl RichText {
    pub fn validate(&self) -> Result<()> {
        if let Some(plain) = &self.plain {
            super::validate(plain, true)?;
        }
        if let Some(data) = &self.html {
            html(data.as_bytes())?;
        }
        if let Some(data) = &self.rtf {
            validate_rtf(data)?;
        }
        if self.html.is_none() && self.rtf.is_none() {
            return Err(invalid("no rich format"));
        }
        Ok(())
    }
    /// Length-delimited, format-separated identity; never persisted or logged.
    pub fn digest(&self) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let mut hash = Sha256::new();
        hash.update(b"rich\0");
        for data in [
            self.plain.as_ref().map(|x| x.as_bytes()),
            self.html.as_ref().map(|x| x.as_bytes()),
            self.rtf.as_ref().map(|x| x.as_slice()),
        ] {
            match data {
                Some(bytes) => {
                    hash.update([1]);
                    hash.update((bytes.len() as u64).to_be_bytes());
                    hash.update(bytes);
                }
                None => hash.update([0]),
            }
        }
        hash.finalize().into()
    }
}
/// CF_HTML uses byte offsets, not UTF-16/character offsets. Its header is not Mac HTML.
pub fn from_cf_html(bytes: &[u8]) -> Result<Zeroizing<String>> {
    if bytes.len() > MAX_RICH + 4096 {
        return Err(invalid("CF_HTML exceeds limit"));
    }
    let header_end = bytes
        .windows(14)
        .position(|w| w == b"StartFragment:")
        .ok_or_else(|| invalid("missing fragment offsets"))?;
    if header_end > 4096 {
        return Err(invalid("CF_HTML header exceeds limit"));
    }
    fn offset(bytes: &[u8], key: &[u8]) -> Result<i64> {
        let mut found = None;
        for line in bytes[..bytes.len().min(4096)].split(|&b| b == b'\r' || b == b'\n') {
            if let Some(value) = line.strip_prefix(key) {
                if found.is_some() {
                    return Err(invalid("duplicate CF_HTML offset"));
                }
                let end = value
                    .iter()
                    .position(|&b| b == b'<' || b == 0)
                    .unwrap_or(value.len());
                found = Some(
                    std::str::from_utf8(&value[..end])
                        .ok()
                        .and_then(|s| s.trim().parse::<i64>().ok())
                        .ok_or_else(|| invalid("invalid CF_HTML offset"))?,
                );
            }
        }
        found.ok_or_else(|| invalid("missing CF_HTML offset"))
    }
    let start = offset(bytes, b"StartFragment:")?;
    let end = offset(bytes, b"EndFragment:")?;
    if start < 0 || end < start || end as usize > bytes.len() {
        return Err(invalid("CF_HTML fragment is out of range"));
    }
    std::str::from_utf8(&bytes[start as usize..end as usize])
        .map_err(|_| invalid("CF_HTML fragment cuts a Unicode character"))?;
    // StartHTML/EndHTML may be -1 (no context), per Microsoft's format.
    let first = offset(bytes, b"StartHTML:")?;
    let last = offset(bytes, b"EndHTML:")?;
    let range = if first == -1 && last == -1 {
        start as usize..end as usize
    } else {
        if first < 0 || last < first || first > start || last < end || last as usize > bytes.len() {
            return Err(invalid("CF_HTML context is out of range"));
        }
        first as usize..last as usize
    };
    html(&bytes[range])
}
pub fn to_cf_html(source: &str) -> Result<Zeroizing<Vec<u8>>> {
    html(source.as_bytes())?;
    let template = "Version:1.0\r\nStartHTML:0000000000\r\nEndHTML:0000000000\r\nStartFragment:0000000000\r\nEndFragment:0000000000\r\n";
    let prefix = "<!--StartFragment-->";
    let suffix = "<!--EndFragment-->";
    let start = template.len();
    let fragment = start + prefix.len();
    let end_fragment = fragment + source.len();
    let end = end_fragment + suffix.len();
    let header = format!(
        "Version:1.0\r\nStartHTML:{start:010}\r\nEndHTML:{end:010}\r\nStartFragment:{fragment:010}\r\nEndFragment:{end_fragment:010}\r\n"
    );
    let mut out = Zeroizing::new(Vec::with_capacity(end + 1));
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(prefix.as_bytes());
    out.extend_from_slice(source.as_bytes());
    out.extend_from_slice(suffix.as_bytes());
    out.push(0);
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mac_html_preserves_unicode_and_is_idempotent() {
        let source = "<html><head><meta charset=gbk></head><body>功能🍎\n  中文</body></html>";
        let wire = mac_html(source).unwrap();
        assert!(wire.starts_with(&[0xef, 0xbb, 0xbf]));
        assert_eq!(&wire[3..], source.as_bytes());
        assert_eq!(*html(&wire).unwrap(), source);
        assert_eq!(
            *mac_html(std::str::from_utf8(&wire).unwrap()).unwrap(),
            *wire
        );
    }
    #[test]
    fn mac_html_limit_counts_content_not_encoding_signature() {
        let source = "a".repeat(MAX_RICH);
        let wire = mac_html(&source).unwrap();
        assert_eq!(wire.len(), MAX_RICH + 3);
        assert_eq!(html(&wire).unwrap().len(), MAX_RICH);
        assert!(mac_html(&(source + "a")).is_err());
        assert!(mac_html("\u{feff}").is_err());
    }
    #[test]
    fn fragment_offsets_cannot_split_unicode_or_escape_the_context() {
        let mut wire = to_cf_html("<b>中</b>").unwrap();
        let index = wire
            .windows(14)
            .position(|w| w == b"StartFragment:")
            .unwrap()
            + 14;
        let character = wire.windows(3).position(|w| w == "中".as_bytes()).unwrap();
        wire[index..index + 10].copy_from_slice(format!("{:010}", character + 1).as_bytes());
        assert!(from_cf_html(&wire).is_err());
        let mut wire = to_cf_html("<b>中</b>").unwrap();
        let index = wire.windows(8).position(|w| w == b"EndHTML:").unwrap() + 8;
        wire[index..index + 10].copy_from_slice(b"0000000001");
        assert!(from_cf_html(&wire).is_err());
    }
    #[test]
    fn cf_html_unicode_offsets_preserve_context_and_fragment() {
        let source =
            "<html><head><style>b{color:red}</style></head><body><b>中文🍎</b></body></html>";
        let wire = to_cf_html(source).unwrap();
        let parsed = from_cf_html(&wire).unwrap();
        assert!(parsed.contains(source));
        let fragment = wire
            .windows(14)
            .position(|w| w == b"StartFragment:")
            .unwrap()
            + 14;
        let offset = std::str::from_utf8(&wire[fragment..fragment + 10])
            .unwrap()
            .parse::<usize>()
            .unwrap();
        assert!(wire[offset..].starts_with(source.as_bytes()));
        let mut invalid = wire.to_vec();
        invalid[fragment..fragment + 10].copy_from_slice(b"9999999999");
        assert!(from_cf_html(&invalid).is_err());
    }
    #[test]
    fn malformed_or_oversized_rich_formats_rejected() {
        assert!(html(b"bad\0html").is_err());
        assert!(html(&[0xff]).is_err());
        assert!(html(&vec![b'a'; MAX_RICH + 1]).is_err());
        assert!(validate_rtf(b"{\\rtf1 test}").is_ok());
        assert!(validate_rtf(b"{\\rtf1 broken").is_err());
        assert!(validate_rtf(b"{\\rtf1 a\0b}").is_err());
        assert!(from_cf_html(b"<b>not a Windows header</b>").is_err());
    }
    #[test]
    fn identities_include_every_format() {
        let mut rich = RichText {
            plain: Some(Zeroizing::new("same".into())),
            html: Some(Zeroizing::new("<b>same</b>".into())),
            rtf: None,
        };
        let old = rich.digest();
        rich.html = Some(Zeroizing::new("<i>same</i>".into()));
        assert_ne!(old, rich.digest());
    }
}
