use super::wire::failed;
use crate::error::Result;
pub fn decode(value: &[u8]) -> Result<String> {
    if value.len() > 32768 {
        return Err(failed("file URL exceeds limit"));
    }
    let uri = url::Url::parse(
        std::str::from_utf8(value).map_err(|_| failed("invalid file URL encoding"))?,
    )
    .map_err(|_| failed("invalid file URL"))?;
    if uri.scheme() != "file"
        || !matches!(uri.host_str(), None | Some("localhost"))
        || uri.query().is_some()
        || uri.fragment().is_some()
        || !uri.username().is_empty()
        || uri.password().is_some()
    {
        return Err(failed("file URL must refer to this Mac"));
    }
    let input = uri.path().as_bytes();
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%' {
            let hex = input
                .get(i + 1..i + 3)
                .ok_or_else(|| failed("invalid file URL escape"))?;
            let digit = |b: u8| match b {
                b'0'..=b'9' => Some(b - b'0'),
                b'A'..=b'F' => Some(b - b'A' + 10),
                b'a'..=b'f' => Some(b - b'a' + 10),
                _ => None,
            };
            out.push(
                digit(hex[0])
                    .zip(digit(hex[1]))
                    .map(|(a, b)| a * 16 + b)
                    .ok_or_else(|| failed("invalid file URL escape"))?,
            );
            i += 3;
        } else {
            out.push(input[i]);
            i += 1;
        }
    }
    let path = String::from_utf8(out).map_err(|_| failed("invalid file path Unicode"))?;
    if !path.starts_with('/')
        || path == "/"
        || path.chars().any(|c| c < ' ')
        || path.split('/').any(|part| matches!(part, "." | ".."))
    {
        return Err(failed("invalid Mac file path"));
    }
    Ok(path)
}
pub fn encode(path: &str) -> Result<String> {
    if !path.starts_with('/') || path == "/" || path.len() > 32768 || path.chars().any(|c| c < ' ')
    {
        return Err(failed("invalid Mac staging path"));
    }
    let mut url = url::Url::parse("file:///").expect("constant file URL");
    url.path_segments_mut()
        .map_err(|_| failed("invalid file URL"))?
        .clear()
        .extend(path.split('/').skip(1));
    Ok(url.to_string())
}
pub fn digest(paths: &[String]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(b"native-files\0");
    for path in paths {
        hash.update((path.len() as u64).to_be_bytes());
        hash.update(path.as_bytes());
    }
    hash.finalize().into()
}
