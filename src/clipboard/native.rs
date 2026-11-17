use windows::Win32::{
    Foundation::{ERROR_SUCCESS, GetLastError, GlobalFree, HANDLE, HGLOBAL, HWND, SetLastError},
    System::{
        DataExchange::{
            CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardOwner,
            GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard,
            SetClipboardData,
        },
        Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock},
    },
    UI::WindowsAndMessaging::GetForegroundWindow,
};
use zeroize::Zeroizing;

use super::MAX_TEXT;
use crate::error::{Error, Result};

const UNICODE_TEXT: u32 = 13;

struct Clipboard(bool);

impl Clipboard {
    fn open(owner: isize) -> Result<Self> {
        unsafe { OpenClipboard(Some(HWND(owner as *mut std::ffi::c_void))) }.map_err(|error| {
            Error::Clipboard(format!("Windows clipboard is busy or unavailable: {error}"))
        })?;
        Ok(Self(true))
    }

    fn close(&mut self) -> Result<()> {
        unsafe { CloseClipboard() }.map_err(|error| Error::Clipboard(error.to_string()))?;
        self.0 = false;
        Ok(())
    }
}

impl Drop for Clipboard {
    fn drop(&mut self) {
        if self.0
            && let Err(error) = unsafe { CloseClipboard() }
        {
            eprintln!("MacCanvas clipboard close failed: {error}");
        }
    }
}

fn unlock(handle: HGLOBAL) -> Result<()> {
    unsafe {
        SetLastError(ERROR_SUCCESS);
    }
    let result = unsafe { GlobalUnlock(handle) };
    match result {
        Ok(()) => Ok(()),
        Err(_) if unsafe { GetLastError() } == ERROR_SUCCESS => Ok(()),
        Err(error) => Err(Error::Clipboard(error.to_string())),
    }
}

// GlobalFree returns NULL on success. windows 0.62 wraps that NULL as Err,
// unlike GlobalAlloc; clear/check last-error rather than treating Err as a failure.
fn free(handle: HGLOBAL) -> Result<()> {
    unsafe {
        SetLastError(ERROR_SUCCESS);
    }
    match unsafe { GlobalFree(Some(handle)) } {
        Err(_) if unsafe { GetLastError() } == ERROR_SUCCESS => Ok(()),
        Err(error) => Err(Error::Clipboard(error.to_string())),
        Ok(_) => Err(Error::Clipboard(
            "Windows could not free an untransferred clipboard allocation".into(),
        )),
    }
}

pub fn sequence() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

pub fn foreground(owner: isize) -> bool {
    unsafe { GetForegroundWindow().0 as isize == owner }
}

pub fn read(owner: isize) -> Result<Option<Zeroizing<String>>> {
    Ok(read_snapshot(owner)?.map(|(_, text)| text))
}

pub fn read_snapshot(owner: isize) -> Result<Option<(u32, Zeroizing<String>)>> {
    let mut clipboard = Clipboard::open(owner)?;
    if unsafe { IsClipboardFormatAvailable(UNICODE_TEXT) }.is_err() {
        clipboard.close()?;
        return Ok(None);
    }
    let handle = unsafe { GetClipboardData(UNICODE_TEXT) }
        .map_err(|error| Error::Clipboard(error.to_string()))?;
    let global = HGLOBAL(handle.0);
    let size = unsafe { GlobalSize(global) };
    if size == 0 || size > (MAX_TEXT * 2 + 1) * 2 || !size.is_multiple_of(2) {
        return Err(Error::Clipboard(
            "Windows clipboard text is malformed or larger than 1 MiB".into(),
        ));
    }
    let pointer = unsafe { GlobalLock(global) };
    if pointer.is_null() {
        return Err(Error::Clipboard(
            "Windows could not lock the clipboard text".into(),
        ));
    }
    let units = unsafe { std::slice::from_raw_parts(pointer.cast::<u16>(), size / 2) };
    let result = match units.iter().position(|&unit| unit == 0) {
        Some(end) => String::from_utf16(&units[..end])
            .map(Zeroizing::new)
            .map_err(|_| Error::Clipboard("Windows clipboard contains invalid UTF-16".into())),
        None => Err(Error::Clipboard(
            "Windows clipboard text has no terminator".into(),
        )),
    };
    unlock(global)?;
    let text = Zeroizing::new(super::normalize(&result?));
    super::validate(&text, true)?;
    let sequence = sequence();
    clipboard.close()?;
    Ok(Some((sequence, text)))
}

pub fn write(owner: isize, text: &str) -> Result<()> {
    write_checked(owner, text, None).map(|_| ())
}

/// The sequence check and replacement occur while the Windows clipboard is exclusively open.
/// Windows may synthesize formats and advance its sequence on CloseClipboard. Reopen
/// read-only and verify ownership before acknowledging that final sequence; a competing
/// application's newer copy wins, including one made immediately after our write.
pub fn write_if_sequence(owner: isize, text: &str, expected: u32) -> Result<Option<u32>> {
    write_checked(owner, text, Some(expected))
}

fn write_checked(owner: isize, text: &str, expected: Option<u32>) -> Result<Option<u32>> {
    super::validate(text, true)?;
    let text = Zeroizing::new(super::normalize(text).replace('\n', "\r\n"));
    let units = Zeroizing::new(text.encode_utf16().chain(Some(0)).collect::<Vec<_>>());
    let allocation = unsafe { GlobalAlloc(GMEM_MOVEABLE, units.len() * 2) }
        .map_err(|error| Error::Clipboard(error.to_string()))?;
    let pointer = unsafe { GlobalLock(allocation) };
    if pointer.is_null() {
        free(allocation)?;
        return Err(Error::Clipboard(
            "Windows could not allocate clipboard text".into(),
        ));
    }
    unsafe {
        std::ptr::copy_nonoverlapping(units.as_ptr(), pointer.cast(), units.len());
    }
    let mut transferred = false;
    let result = (|| {
        unlock(allocation)?;
        let mut clipboard = Clipboard::open(owner)?;
        if expected.is_some_and(|expected| sequence() != expected) {
            clipboard.close()?;
            return Ok(None);
        }
        unsafe { EmptyClipboard() }.map_err(|error| Error::Clipboard(error.to_string()))?;
        unsafe { SetClipboardData(UNICODE_TEXT, Some(HANDLE(allocation.0))) }
            .map_err(|error| Error::Clipboard(error.to_string()))?;
        transferred = true;
        clipboard.close()?;
        if expected.is_none() {
            return Ok(Some(0));
        } // Manual writes do not need an echo receipt.
        let mut receipt = Clipboard::open(owner)?;
        let still_owned =
            unsafe { GetClipboardOwner() }.is_ok_and(|window| window.0 as isize == owner);
        let written = still_owned.then(sequence);
        receipt.close()?;
        Ok(written)
    })();
    if !transferred {
        free(allocation)?;
    }
    result
}

#[derive(Default)]
pub struct Watcher {
    sent: Option<u32>,
    remote: Option<u32>,
}

impl Watcher {
    pub fn paste(&self, owner: isize) -> Result<Option<(u32, Zeroizing<String>)>> {
        let sequence = sequence();
        if self.remote == Some(sequence) || self.sent == Some(sequence) {
            return Ok(None);
        }
        read_snapshot(owner)?.map(Some).ok_or_else(|| {
            Error::Clipboard(
                "Only plain-text clipboard sharing is supported; nothing was pasted".into(),
            )
        })
    }

    pub fn sent(&mut self, sequence: u32) {
        self.sent = Some(sequence);
    }

    pub fn receive(&mut self, owner: isize, text: &str) -> Result<()> {
        write(owner, text)?;
        let sequence = sequence();
        self.remote = Some(sequence);
        Ok(())
    }
}

/// Fail closed on secure/locked desktops and disconnected/switched Windows sessions.
/// Background application windows on the normal interactive desktop remain eligible.
pub fn interactive_session() -> bool {
    use windows::Win32::{
        System::RemoteDesktop::{
            WTS_CURRENT_SERVER_HANDLE, WTS_CURRENT_SESSION, WTSActive, WTSConnectState,
            WTSFreeMemory, WTSQuerySessionInformationW,
        },
        System::StationsAndDesktops::{
            CloseDesktop, DESKTOP_CONTROL_FLAGS, DESKTOP_READOBJECTS, GetUserObjectInformationW,
            OpenInputDesktop, UOI_NAME,
        },
    };
    unsafe {
        let mut info = windows::core::PWSTR::null();
        let mut bytes = 0;
        if WTSQuerySessionInformationW(
            Some(WTS_CURRENT_SERVER_HANDLE),
            WTS_CURRENT_SESSION,
            WTSConnectState,
            &mut info,
            &mut bytes,
        )
        .is_err()
        {
            return false;
        }
        let active =
            bytes as usize >= std::mem::size_of::<i32>() && *info.0.cast::<i32>() == WTSActive.0;
        WTSFreeMemory(info.0.cast());
        if !active {
            return false;
        }
        let Ok(desktop) = OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_READOBJECTS)
        else {
            return false;
        };
        let mut name = [0u16; 64];
        let mut needed = 0;
        let read = GetUserObjectInformationW(
            HANDLE(desktop.0),
            UOI_NAME,
            Some(name.as_mut_ptr().cast()),
            std::mem::size_of_val(&name) as u32,
            Some(&mut needed),
        );
        let _ = CloseDesktop(desktop);
        read.is_ok()
            && name.split(|&c| c == 0).next() == Some(&[68, 101, 102, 97, 117, 108, 116][..])
    }
}

/// Native image APIs are separate from the legacy text/SSH clipboard path.
pub enum ImageSource {
    Tiff(Zeroizing<Vec<u8>>),
    Png(Zeroizing<Vec<u8>>),
    Dib(Zeroizing<Vec<u8>>),
}
fn png_format() -> Result<u32> {
    use windows::{Win32::System::DataExchange::RegisterClipboardFormatW, core::w};
    let format = unsafe { RegisterClipboardFormatW(w!("PNG")) };
    if format == 0 {
        return Err(Error::Clipboard(
            "Could not register PNG clipboard format".into(),
        ));
    }
    Ok(format)
}
pub fn read_image_snapshot(owner: isize) -> Result<Option<(u32, ImageSource)>> {
    let mut clipboard = Clipboard::open(owner)?;
    // File copies sometimes advertise a preview bitmap; never transmit that as the file.
    if unsafe { IsClipboardFormatAvailable(15) }.is_ok() {
        clipboard.close()?;
        return Err(Error::Clipboard(
            "File clipboard transfer is not implemented".into(),
        ));
    }
    let png = png_format()?;
    let format = [png, 17, 8, 6]
        .into_iter()
        .find(|&format| unsafe { IsClipboardFormatAvailable(format) }.is_ok());
    let Some(format) = format else {
        clipboard.close()?;
        return Ok(None);
    };
    let handle =
        unsafe { GetClipboardData(format) }.map_err(|e| Error::Clipboard(e.to_string()))?;
    let global = HGLOBAL(handle.0);
    let size = unsafe { GlobalSize(global) };
    let limit = if format == png {
        super::image::MAX_PNG
    } else if format == 6 {
        super::image::MAX_TIFF
    } else {
        super::image::MAX_DIB
    };
    if size == 0 || size > limit {
        return Err(Error::Clipboard(
            "Clipboard image allocation exceeds limit".into(),
        ));
    }
    let pointer = unsafe { GlobalLock(global) };
    if pointer.is_null() {
        return Err(Error::Clipboard("Could not lock clipboard image".into()));
    }
    let mut bytes =
        Zeroizing::new(unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), size) }.to_vec());
    unlock(global)?;
    let seq = sequence();
    clipboard.close()?;
    if format == png {
        let length = super::image::png_length(&bytes)?;
        bytes.truncate(length);
    }
    Ok(Some((
        seq,
        if format == png {
            ImageSource::Png(bytes)
        } else if format == 6 {
            ImageSource::Tiff(bytes)
        } else {
            ImageSource::Dib(bytes)
        },
    )))
}
fn allocate_bytes(bytes: &[u8]) -> Result<HGLOBAL> {
    let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) }
        .map_err(|e| Error::Clipboard(e.to_string()))?;
    let pointer = unsafe { GlobalLock(handle) };
    if pointer.is_null() {
        free(handle)?;
        return Err(Error::Clipboard(
            "Could not allocate clipboard image".into(),
        ));
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer.cast(), bytes.len());
    }
    if let Err(error) = unlock(handle) {
        free(handle)?;
        return Err(error);
    }
    Ok(handle)
}
/// Buffers are fully decoded and allocated before replacing the clipboard.
/// Only called on the UI thread after the image worker's epoch and sequence are checked.
pub fn write_image_if_sequence(
    owner: isize,
    png: &[u8],
    dib: &[u8],
    expected: u32,
) -> Result<Option<u32>> {
    super::image::validate_png(png)?;
    if dib.len() > super::image::MAX_DIB {
        return Err(Error::Clipboard("Clipboard image exceeds limit".into()));
    }
    let format = png_format()?;
    let png_memory = allocate_bytes(png)?;
    let dib_memory = match allocate_bytes(dib) {
        Ok(memory) => memory,
        Err(error) => {
            free(png_memory)?;
            return Err(error);
        }
    };
    let mut png_transferred = false;
    let mut dib_transferred = false;
    let result = (|| {
        let mut clipboard = Clipboard::open(owner)?;
        if sequence() != expected {
            clipboard.close()?;
            return Ok(None);
        }
        unsafe { EmptyClipboard() }.map_err(|e| Error::Clipboard(e.to_string()))?;
        unsafe { SetClipboardData(format, Some(HANDLE(png_memory.0))) }
            .map_err(|e| Error::Clipboard(e.to_string()))?;
        png_transferred = true;
        unsafe { SetClipboardData(17, Some(HANDLE(dib_memory.0))) }
            .map_err(|e| Error::Clipboard(e.to_string()))?;
        dib_transferred = true;
        clipboard.close()?;
        let mut receipt = Clipboard::open(owner)?;
        let owned = unsafe { GetClipboardOwner() }.is_ok_and(|window| window.0 as isize == owner);
        let written = owned.then(sequence);
        receipt.close()?;
        Ok(written)
    })();
    if !png_transferred {
        free(png_memory)?;
    }
    if !dib_transferred {
        free(dib_memory)?;
    }
    result
}

fn rich_formats() -> Result<(u32, u32)> {
    use windows::{Win32::System::DataExchange::RegisterClipboardFormatW, core::w};
    let html = unsafe { RegisterClipboardFormatW(w!("HTML Format")) };
    let rtf = unsafe { RegisterClipboardFormatW(w!("Rich Text Format")) };
    if html == 0 || rtf == 0 {
        return Err(Error::Clipboard(
            "Could not register rich clipboard formats".into(),
        ));
    }
    Ok((html, rtf))
}
/// Caller owns the clipboard lock; data is copied out before any conversion.
fn read_format(format: u32, limit: usize) -> Result<Option<Zeroizing<Vec<u8>>>> {
    if unsafe { IsClipboardFormatAvailable(format) }.is_err() {
        return Ok(None);
    }
    let handle =
        unsafe { GetClipboardData(format) }.map_err(|e| Error::Clipboard(e.to_string()))?;
    let global = HGLOBAL(handle.0);
    let size = unsafe { GlobalSize(global) };
    if size == 0 || size > limit {
        return Ok(None);
    }
    let pointer = unsafe { GlobalLock(global) };
    if pointer.is_null() {
        return Err(Error::Clipboard(
            "Could not lock rich clipboard data".into(),
        ));
    }
    let bytes =
        Zeroizing::new(unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), size) }.to_vec());
    unlock(global)?;
    Ok(Some(bytes))
}
pub fn read_rich_snapshot(owner: isize) -> Result<Option<(u32, super::rich::RichText)>> {
    use super::rich::{MAX_RICH, RichText};
    let mut clipboard = Clipboard::open(owner)?;
    if unsafe { IsClipboardFormatAvailable(15) }.is_ok() {
        clipboard.close()?;
        return Ok(None);
    }
    let (html_format, rtf_format) = rich_formats()?;
    let mut rich = RichText::default();
    if let Some(bytes) = read_format(html_format, MAX_RICH + 4096)? {
        rich.html = super::rich::from_cf_html(&bytes).ok();
    }
    if let Some(mut bytes) = read_format(rtf_format, MAX_RICH + 1)? {
        if let Some(end) = bytes.iter().position(|&b| b == 0) {
            bytes.truncate(end);
        }
        if super::rich::validate_rtf(&bytes).is_ok() {
            rich.rtf = Some(bytes);
        }
    }
    if rich.html.is_none() && rich.rtf.is_none() {
        clipboard.close()?;
        return Ok(None);
    }
    if let Some(bytes) = read_format(UNICODE_TEXT, (MAX_TEXT + 1) * 2)?
        && bytes.len().is_multiple_of(2)
    {
        let units = Zeroizing::new(
            bytes
                .chunks_exact(2)
                .map(|x| u16::from_le_bytes(x.try_into().unwrap()))
                .collect::<Vec<_>>(),
        );
        if let Some(end) = units.iter().position(|&x| x == 0)
            && let Ok(text) = String::from_utf16(&units[..end])
        {
            let text = Zeroizing::new(text);
            let text = Zeroizing::new(super::normalize(&text));
            if !text.is_empty() && super::validate(&text, true).is_ok() {
                rich.plain = Some(text);
            }
        }
    }
    let seq = sequence();
    clipboard.close()?;
    Ok(Some((seq, rich)))
}
pub fn write_rich_if_sequence(
    owner: isize,
    rich: &super::rich::RichText,
    expected: u32,
) -> Result<Option<u32>> {
    rich.validate()?;
    let (html_format, rtf_format) = rich_formats()?;
    let mut formats = Vec::new();
    if let Some(plain) = &rich.plain {
        let units = Zeroizing::new(
            super::normalize(plain)
                .replace('\n', "\r\n")
                .encode_utf16()
                .chain(Some(0))
                .collect::<Vec<_>>(),
        );
        let bytes = Zeroizing::new(
            units
                .iter()
                .flat_map(|x| x.to_le_bytes())
                .collect::<Vec<_>>(),
        );
        formats.push((UNICODE_TEXT, bytes));
    }
    if let Some(html) = &rich.html {
        formats.push((html_format, super::rich::to_cf_html(html)?));
    }
    if let Some(rtf) = &rich.rtf {
        let mut bytes = rtf.clone();
        bytes.push(0);
        formats.push((rtf_format, bytes));
    }
    write_formats_if_sequence(owner, &formats, expected)
}
fn write_formats_if_sequence(
    owner: isize,
    formats: &[(u32, Zeroizing<Vec<u8>>)],
    expected: u32,
) -> Result<Option<u32>> {
    // Allocate everything first. Each successful SetClipboardData transfers ownership to Windows.
    let mut allocations = Vec::new();
    for (format, bytes) in formats {
        match allocate_bytes(bytes) {
            Ok(memory) => allocations.push((*format, memory, false)),
            Err(error) => {
                for (_, memory, _) in allocations {
                    free(memory)?;
                }
                return Err(error);
            }
        }
    }
    let result = (|| {
        let mut clipboard = Clipboard::open(owner)?;
        if sequence() != expected {
            clipboard.close()?;
            return Ok(None);
        }
        unsafe { EmptyClipboard() }.map_err(|e| Error::Clipboard(e.to_string()))?;
        for (format, memory, transferred) in &mut allocations {
            unsafe { SetClipboardData(*format, Some(HANDLE(memory.0))) }
                .map_err(|e| Error::Clipboard(e.to_string()))?;
            *transferred = true;
        }
        clipboard.close()?;
        let mut receipt = Clipboard::open(owner)?;
        let owned = unsafe { GetClipboardOwner() }.is_ok_and(|window| window.0 as isize == owner);
        let written = owned.then(sequence);
        receipt.close()?;
        Ok(written)
    })();
    for (_, memory, transferred) in allocations {
        if !transferred {
            free(memory)?;
        }
    }
    result
}

pub fn read_files_snapshot(owner: isize) -> Result<Option<(u32, Vec<std::path::PathBuf>)>> {
    let mut clipboard = Clipboard::open(owner)?;
    let data = read_format(15, super::filecopy::dropfiles::MAX_LIST)?;
    let result = data
        .map(|bytes| super::filecopy::dropfiles::decode(&bytes))
        .transpose()?;
    let seq = sequence();
    clipboard.close()?;
    Ok(result.map(|paths| {
        (
            seq,
            paths.into_iter().map(std::path::PathBuf::from).collect(),
        )
    }))
}
pub fn write_files_if_sequence(
    owner: isize,
    paths: &[std::path::PathBuf],
    expected: u32,
) -> Result<Option<u32>> {
    use windows::{Win32::System::DataExchange::RegisterClipboardFormatW, core::w};
    let paths = paths
        .iter()
        .map(|path| {
            if !path.is_absolute() {
                return Err(Error::Clipboard("Staged file path is not absolute".into()));
            }
            path.to_str()
                .map(str::to_owned)
                .ok_or_else(|| Error::Clipboard("Invalid staged path Unicode".into()))
        })
        .collect::<Result<Vec<_>>>()?;
    let format = unsafe { RegisterClipboardFormatW(w!("Preferred DropEffect")) };
    if format == 0 {
        return Err(Error::Clipboard("Cannot register file copy format".into()));
    }
    let formats = vec![
        (15, super::filecopy::dropfiles::encode(&paths)?),
        (format, Zeroizing::new(1u32.to_le_bytes().to_vec())),
    ];
    write_formats_if_sequence(owner, &formats, expected)
}

pub fn owns_clipboard(owner: isize) -> bool {
    unsafe { GetClipboardOwner() }.is_ok_and(|window| window.0 as isize == owner)
}
