//! CF_HDROP memory layout, independent of the Windows clipboard API.
use super::wire::failed;
use crate::error::Result;
use zeroize::Zeroizing;
pub const MAX_LIST: usize = 128 * 1024;
pub fn decode(bytes: &[u8]) -> Result<Vec<String>> {
    if bytes.len() < 24 || bytes.len() > MAX_LIST || bytes[16..20] != 1u32.to_le_bytes() {
        return Err(failed("unsupported file clipboard format"));
    }
    let offset = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
    if offset < 20
        || !offset.is_multiple_of(2)
        || offset >= bytes.len()
        || !(bytes.len() - offset).is_multiple_of(2)
    {
        return Err(failed("invalid file clipboard offset"));
    }
    let units = Zeroizing::new(
        bytes[offset..]
            .chunks_exact(2)
            .map(|p| u16::from_le_bytes(p.try_into().unwrap()))
            .collect::<Vec<_>>(),
    );
    let mut paths = Vec::new();
    let mut at = 0;
    while at < units.len() {
        let length = units[at..]
            .iter()
            .position(|&x| x == 0)
            .ok_or_else(|| failed("unterminated file list"))?;
        if length == 0 {
            if paths.is_empty() {
                return Err(failed("empty file list"));
            }
            return Ok(paths);
        }
        if paths.len() >= 16 || length > 32767 {
            return Err(failed("file list exceeds limit"));
        }
        paths.push(
            String::from_utf16(&units[at..at + length])
                .map_err(|_| failed("invalid file name Unicode"))?,
        );
        at += length + 1;
    }
    Err(failed("file list lacks double terminator"))
}
pub fn encode(paths: &[String]) -> Result<Zeroizing<Vec<u8>>> {
    if paths.is_empty() || paths.len() > 16 {
        return Err(failed("invalid file root count"));
    }
    let mut out = Zeroizing::new(vec![0; 20]);
    out[..4].copy_from_slice(&20u32.to_le_bytes());
    out[16..20].copy_from_slice(&1u32.to_le_bytes());
    for path in paths {
        if path.is_empty() || path.contains('\0') || path.encode_utf16().count() > 32767 {
            return Err(failed("invalid file path"));
        }
        for unit in path.encode_utf16().chain(Some(0)) {
            out.extend_from_slice(&unit.to_le_bytes());
        }
        if out.len() > MAX_LIST - 2 {
            return Err(failed("file list exceeds limit"));
        }
    }
    out.extend_from_slice(&0u16.to_le_bytes());
    Ok(out)
}
