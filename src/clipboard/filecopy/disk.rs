use super::wire::{ITEM, Message, failed};
use crate::error::Result;
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
pub const MAX_ITEMS: usize = 4096;
pub const MAX_DEPTH: usize = 64;
pub const MAX_BYTES: u64 = 256 * 1024 * 1024;
pub fn component(name: &str) -> Result<()> {
    let base = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    let device = matches!(
        base.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$"
    ) || ["COM", "LPT"].iter().any(|prefix| {
        base.strip_prefix(prefix).is_some_and(|tail| {
            matches!(
                tail,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    });
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.encode_utf16().count() > 255
        || name.ends_with(['.', ' '])
        || name
            .chars()
            .any(|c| c < ' ' || matches!(c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*'))
        || device
    {
        return Err(failed("unsafe or unsupported file name"));
    }
    Ok(())
}
fn regular(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return false;
        }
    }
    !metadata.file_type().is_symlink()
}
fn guard(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options
            .access_mode(0x80)
            .share_mode(3)
            .custom_flags(0x00200000 | 0x02000000);
    }
    let file = options
        .open(path)
        .map_err(|_| failed("cannot lock transfer directory"))?;
    let metadata = file
        .metadata()
        .map_err(|_| failed("cannot inspect transfer directory"))?;
    if !metadata.is_dir() || !regular(&metadata) {
        return Err(failed("links and reparse points are not supported"));
    }
    Ok(file)
}
fn ancestors(path: &Path, guards: &mut Vec<File>) -> Result<()> {
    let mut nodes = path.ancestors().collect::<Vec<_>>();
    nodes.reverse();
    for node in nodes {
        if !node.as_os_str().is_empty() {
            guards.push(guard(node)?);
        }
    }
    Ok(())
}
pub struct Item {
    pub name: String,
    pub level: u16,
    pub directory: bool,
    pub size: u64,
    pub source: PathBuf,
    pub modified: Option<std::time::SystemTime>,
}
impl Item {
    pub fn message(&self, session: u32) -> Message {
        let mut body = vec![0; 104];
        body[0] = if self.directory { 2 } else { 1 };
        body[42..50].copy_from_slice(&self.size.to_be_bytes());
        if self.directory {
            body[90..92].copy_from_slice(&0x10u16.to_be_bytes());
        }
        body[92..94].copy_from_slice(&self.level.to_be_bytes());
        body[94..96].copy_from_slice(
            &(if self.directory {
                0o40700u16
            } else {
                0o100600u16
            })
            .to_be_bytes(),
        );
        body[100..102].copy_from_slice(&(self.name.len() as u16).to_be_bytes());
        body.extend_from_slice(self.name.as_bytes());
        body.push(0);
        Message::new(ITEM, session, body)
    }
    pub fn open(&self) -> Result<File> {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(1).custom_flags(0x00200000);
        }
        let file = options
            .open(&self.source)
            .map_err(|_| failed("source file is busy or unavailable"))?;
        let metadata = file
            .metadata()
            .map_err(|_| failed("cannot inspect source file"))?;
        if !metadata.is_file()
            || !regular(&metadata)
            || metadata.len() != self.size
            || metadata.modified().ok() != self.modified
        {
            return Err(failed("source changed during transfer"));
        }
        Ok(file)
    }
}
pub struct Plan {
    pub items: Vec<Item>,
    pub bytes: u64,
    _guards: Vec<File>,
}
impl Plan {
    pub fn new(paths: &[PathBuf], root_name: &str, cancelled: impl Fn() -> bool) -> Result<Self> {
        if cancelled() {
            return Err(failed("cancelled"));
        }
        component(root_name)?;
        if paths.is_empty() || paths.len() > 16 {
            return Err(failed("select between 1 and 16 roots"));
        }
        let mut plan = Self {
            items: vec![Item {
                name: root_name.into(),
                level: 0,
                directory: true,
                size: 0,
                source: PathBuf::new(),
                modified: None,
            }],
            bytes: 0,
            _guards: vec![],
        };
        let mut seen = HashSet::new();
        for path in paths {
            if !path.is_absolute() {
                return Err(failed("source must be a local absolute path"));
            }
            #[cfg(windows)]
            {
                use std::path::{Component, Prefix};
                if !matches!(path.components().next(),Some(Component::Prefix(prefix)) if matches!(prefix.kind(),Prefix::Disk(_)|Prefix::VerbatimDisk(_)))
                {
                    return Err(failed("network and device paths are unsupported"));
                }
            }
            let name = path
                .file_name()
                .and_then(|x| x.to_str())
                .ok_or_else(|| failed("invalid source name"))?;
            component(name)?;
            if !seen.insert(name.to_lowercase()) {
                return Err(failed("duplicate root names"));
            }
            if let Some(parent) = path.parent() {
                ancestors(parent, &mut plan._guards)?;
            }
            plan.add(path, 1, &cancelled)?;
        }
        Ok(plan)
    }
    fn add(&mut self, path: &Path, level: usize, cancelled: &impl Fn() -> bool) -> Result<()> {
        if cancelled() {
            return Err(failed("cancelled"));
        }
        if level > MAX_DEPTH || self.items.len() >= MAX_ITEMS {
            return Err(failed("directory tree exceeds limit"));
        }
        let metadata = fs::symlink_metadata(path).map_err(|_| failed("cannot inspect source"))?;
        if !regular(&metadata) || (!metadata.is_dir() && !metadata.is_file()) {
            return Err(failed("links and special files are unsupported"));
        }
        let name = path
            .file_name()
            .and_then(|x| x.to_str())
            .ok_or_else(|| failed("invalid source name"))?;
        component(name)?;
        let size = if metadata.is_file() {
            metadata.len()
        } else {
            0
        };
        self.bytes = self
            .bytes
            .checked_add(size)
            .filter(|&n| n <= MAX_BYTES)
            .ok_or_else(|| failed("batch exceeds 256 MiB"))?;
        self.items.push(Item {
            name: name.into(),
            level: level as u16,
            directory: metadata.is_dir(),
            size,
            source: path.to_owned(),
            modified: metadata.modified().ok(),
        });
        if metadata.is_dir() {
            self._guards.push(guard(path)?);
            let mut names = HashSet::new();
            for child in fs::read_dir(path).map_err(|_| failed("cannot read source directory"))? {
                let child = child.map_err(|_| failed("cannot read directory entry"))?;
                let name = child
                    .file_name()
                    .to_str()
                    .ok_or_else(|| failed("invalid source name Unicode"))?
                    .to_lowercase();
                if !names.insert(name) {
                    return Err(failed("case-colliding source names are unsupported"));
                }
                self.add(&child.path(), level + 1, cancelled)?;
            }
        }
        Ok(())
    }
}
pub struct Receiver {
    root: PathBuf,
    folders: Vec<PathBuf>,
    seen: HashSet<PathBuf>,
    file: Option<File>,
    remaining: u64,
    resource: u64,
    pub bytes: u64,
    pub received: u64,
    pub items: usize,
    pub first: Option<PathBuf>,
    limit: u64,
    _guards: Vec<File>,
}
impl Receiver {
    pub fn new(root: PathBuf, limit: u64) -> Result<Self> {
        let guards = vec![guard(&root)?];
        Ok(Self {
            folders: vec![root.clone()],
            root,
            seen: HashSet::new(),
            file: None,
            remaining: 0,
            resource: 0,
            bytes: 0,
            received: 0,
            items: 0,
            first: None,
            limit,
            _guards: guards,
        })
    }
    pub fn item(&mut self, body: &[u8]) -> Result<()> {
        if self.remaining + self.resource != 0 || body.len() < 105 {
            return Err(failed("truncated or out-of-order item"));
        }
        let directory = match body[0] {
            1 => false,
            2 => true,
            _ => return Err(failed("links and unknown item types are unsupported")),
        };
        let size = u64::from_be_bytes(body[42..50].try_into().unwrap());
        let resource = u64::from_be_bytes(body[34..42].try_into().unwrap());
        let level = u16::from_be_bytes(body[92..94].try_into().unwrap()) as usize;
        let length = u16::from_be_bytes(body[100..102].try_into().unwrap()) as usize;
        let raw = body
            .get(104..104 + length)
            .ok_or_else(|| failed("truncated item name"))?;
        if body.get(104 + length) != Some(&0) {
            return Err(failed("invalid item name termination"));
        }
        let name = std::str::from_utf8(raw)
            .map_err(|_| failed("invalid item name encoding"))?
            .replace('/', ":");
        component(&name)?;
        if level >= self.folders.len()
            || level > MAX_DEPTH
            || (level == 0 && self.first.is_some())
            || self.items >= MAX_ITEMS
        {
            return Err(failed("invalid directory hierarchy"));
        }
        let addition = size
            .checked_add(resource)
            .ok_or_else(|| failed("item length overflow"))?;
        if directory && addition != 0 {
            return Err(failed("directory carries file data"));
        }
        self.bytes = self
            .bytes
            .checked_add(addition)
            .filter(|&n| n <= self.limit)
            .ok_or_else(|| failed("download exceeds batch limit"))?;
        let target = self.folders[level].join(name);
        if !target.starts_with(&self.root)
            || !self
                .seen
                .insert(PathBuf::from(target.to_string_lossy().to_lowercase()))
        {
            return Err(failed("duplicate or escaping destination"));
        }
        self.folders.truncate(level + 1);
        if directory {
            fs::create_dir(&target)
                .map_err(|_| failed("destination exists or cannot be created"))?;
            self._guards.push(guard(&target)?);
            self.folders.push(target.clone());
        } else {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                options.share_mode(1).custom_flags(0x00200000);
            }
            self.file = Some(
                options
                    .open(&target)
                    .map_err(|_| failed("destination exists or cannot be created"))?,
            );
        }
        self.items += 1;
        if level == 0 {
            self.first = Some(target);
        }
        self.remaining = size;
        self.resource = resource;
        if size == 0 {
            self.file = None;
        }
        Ok(())
    }
    pub fn outstanding(&self) -> u64 {
        self.remaining + self.resource
    }
    pub fn data(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.len() as u64 > self.outstanding() {
            return Err(failed("data exceeds declared item size"));
        }
        let count = (self.remaining.min(bytes.len() as u64)) as usize;
        if count != 0 {
            self.file
                .as_mut()
                .ok_or_else(|| failed("data without a file"))?
                .write_all(&bytes[..count])
                .map_err(|_| failed("cannot write staged file"))?;
            self.remaining -= count as u64;
        }
        self.resource -= (bytes.len() - count) as u64;
        self.received += bytes.len() as u64;
        if self.remaining == 0
            && let Some(file) = self.file.take()
        {
            file.sync_all()
                .map_err(|_| failed("cannot finish staged file"))?;
        }
        Ok(())
    }
    pub fn finish(&mut self) -> Result<PathBuf> {
        if self.outstanding() != 0 || self.file.is_some() {
            return Err(failed("incomplete file transfer"));
        }
        self.first
            .clone()
            .ok_or_else(|| failed("server sent no files"))
    }
}

/// Protect the staging root even if TEMP was redirected to a shared directory.
pub fn protect_staging(path: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        use windows::{
            Win32::{
                Foundation::{HLOCAL, LocalFree},
                Security::{
                    Authorization::{
                        ConvertStringSecurityDescriptorToSecurityDescriptorW, SE_FILE_OBJECT,
                        SetNamedSecurityInfoW,
                    },
                    DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl,
                    PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
                },
            },
            core::{BOOL, HSTRING, w},
        };
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                w!("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;OW)"),
                1,
                &mut descriptor,
                None,
            )
        }
        .map_err(|_| failed("cannot create private staging permissions"))?;
        let result = (|| {
            let mut present = BOOL::default();
            let mut defaulted = BOOL::default();
            let mut acl = std::ptr::null_mut();
            unsafe {
                GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted)
            }
            .map_err(|_| failed("invalid staging permissions"))?;
            if !present.as_bool() || acl.is_null() {
                return Err(failed("missing staging permissions"));
            }
            let name = HSTRING::from(path.as_os_str());
            let status = unsafe {
                SetNamedSecurityInfoW(
                    &name,
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    None,
                    None,
                    Some(acl),
                    None,
                )
            };
            if status.0 != 0 {
                return Err(failed("cannot protect staging directory"));
            }
            Ok(())
        })();
        unsafe { LocalFree(Some(HLOCAL(descriptor.0))) };
        result
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| failed("cannot protect staging directory"))
    }
}
