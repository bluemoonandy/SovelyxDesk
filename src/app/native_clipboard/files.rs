use super::*;
use maccanvas::clipboard::filecopy::{self, Connection, StagedFiles};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
static FILE_BUSY: AtomicBool = AtomicBool::new(false);
struct Slot;
impl Slot {
    fn reserve() -> Result<Self> {
        FILE_BUSY
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| Error::Clipboard("A previous file worker is still stopping".into()))?;
        Ok(Self)
    }
}
impl Drop for Slot {
    fn drop(&mut self) {
        FILE_BUSY.store(false, Ordering::Release);
    }
}
pub(super) enum Outcome {
    Uploaded {
        paths: Vec<String>,
        message: maccanvas::clipboard::apple::FileMessage,
    },
    Downloaded(StagedFiles),
}
pub(super) struct Job {
    pub epoch: u64,
    pub sequence: u32,
    pub upload: bool,
    pub cancelled: Arc<AtomicBool>,
    pub completed: Arc<AtomicU64>,
    pub total: Arc<AtomicU64>,
    pub handle: std::thread::JoinHandle<Result<Outcome>>,
}
fn port(state: &State) -> Result<maccanvas::session::NativeFilePort> {
    state
        .session
        .as_ref()
        .ok_or_else(|| Error::Clipboard("No active connection".into()))?
        .native_file_port(state.native_clipboard.epoch)
}
pub(super) fn start_upload(state: &mut State, paths: Vec<std::path::PathBuf>) -> Result<()> {
    let slot = Slot::reserve()?;
    let mut port = port(state)?;
    let cancel = port.cancellation();
    let completed = Arc::new(AtomicU64::new(0));
    let total = Arc::new(AtomicU64::new(0));
    let (done, all) = (completed.clone(), total.clone());
    let root = format!("maccanvas-{}", Uuid::new_v4().simple());
    let handle = std::thread::Builder::new()
        .name("native-files-upload".into())
        .spawn(move || {
            let _slot = slot;
            let paths = filecopy::upload(&mut port, &paths, &root, |a, b| {
                done.store(a, Ordering::Release);
                all.store(b, Ordering::Release);
            })?;
            if port.cancelled() {
                return Err(Error::Clipboard("File transfer cancelled".into()));
            }
            let message = maccanvas::clipboard::apple::FileMessage::new(&paths)?;
            Ok(Outcome::Uploaded { paths, message })
        })
        .map_err(|_| Error::Clipboard("Could not start file worker".into()))?;
    state.native_clipboard.file_task = Some(Job {
        epoch: state.native_clipboard.epoch,
        sequence: state.native_clipboard.sequence,
        upload: true,
        cancelled: cancel,
        completed,
        total,
        handle,
    });
    Ok(())
}
pub(super) fn start_download(state: &mut State, paths: Vec<String>, sequence: u32) -> Result<()> {
    if state.native_clipboard.staged.len() >= 128
        || state
            .native_clipboard
            .staged
            .iter()
            .map(|x| x.bytes)
            .sum::<u64>()
            >= 1024 * 1024 * 1024
    {
        return Err(Error::Clipboard(
            "File staging cache is full; finish pasting and restart the app".into(),
        ));
    }
    let slot = Slot::reserve()?;
    let mut port = port(state)?;
    let cancel = port.cancellation();
    let completed = Arc::new(AtomicU64::new(0));
    let total = Arc::new(AtomicU64::new(0));
    let (done, all) = (completed.clone(), total.clone());
    let handle = std::thread::Builder::new()
        .name("native-files-download".into())
        .spawn(move || {
            let _slot = slot;
            filecopy::download(&mut port, &paths, |a, b| {
                done.store(a, Ordering::Release);
                all.store(b, Ordering::Release);
            })
            .map(Outcome::Downloaded)
        })
        .map_err(|_| Error::Clipboard("Could not start file worker".into()))?;
    state.native_clipboard.file_task = Some(Job {
        epoch: state.native_clipboard.epoch,
        sequence,
        upload: false,
        cancelled: cancel,
        completed,
        total,
        handle,
    });
    Ok(())
}
pub(super) fn poll(
    app: &App,
    state: &mut State,
    active: bool,
    send: bool,
    receive: bool,
    changed: bool,
) -> Result<()> {
    let Some(job) = &state.native_clipboard.file_task else {
        return Ok(());
    };
    if !active
        || changed
        || job.sequence != native::sequence()
        || job.epoch != state.native_clipboard.epoch
        || !(if job.upload { send } else { receive })
        || !app.get_native_clipboard_files()
    {
        job.cancelled.store(true, Ordering::Release);
    }
    if !job.handle.is_finished() {
        if active && !job.cancelled.load(Ordering::Acquire) {
            app.set_native_clipboard_status(
                format!(
                    "Native files: {} / {} KiB (copy only)",
                    job.completed.load(Ordering::Acquire) / 1024,
                    job.total.load(Ordering::Acquire) / 1024
                )
                .into(),
            );
        }
        return Ok(());
    }
    let job = state.native_clipboard.file_task.take().unwrap();
    let completed = job.completed.load(Ordering::Acquire);
    let result = job
        .handle
        .join()
        .unwrap_or_else(|_| Err(Error::Clipboard("File worker failed".into())));
    state.audit.event("clipboard.apple.files.completed",Level::Info,json!({"clipboard.epoch":job.epoch,"transfer.direction":if job.upload{"windows_to_mac"}else{"mac_to_windows"},"transfer.bytes":completed,"transfer.stage":"data_transfer","event.outcome":if result.is_ok(){"complete"}else{"failed"},"network.encrypted":true}))?;
    if !active
        || job.cancelled.load(Ordering::Acquire)
        || job.epoch != state.native_clipboard.epoch
        || job.sequence != native::sequence()
        || !native::interactive_session()
    {
        return Ok(());
    }
    match result {
        Ok(Outcome::Uploaded{paths,message})=>{
            state.native_clipboard.file_echo=Some(filecopy::urls::digest(&paths));
            post(state,NativeCommand::SendFiles{epoch:job.epoch,message})?;
            app.set_native_clipboard_status("File data uploaded; file URLs queued on encrypted control — verify by pasting on the Mac".into());
        }
        Ok(Outcome::Downloaded(staged))=>{
            if state.native_clipboard.staged.iter().map(|x|x.bytes).sum::<u64>()+staged.bytes>1024*1024*1024{return Err(Error::Clipboard("File staging cache exceeds 1 GiB".into()));}
            if let Some(written)=native::write_files_if_sequence(window_handle(app)?,&staged.paths,job.sequence)? {
                let hash=filecopy::urls::digest(&staged.paths.iter().map(|path|path.to_string_lossy().into_owned()).collect::<Vec<_>>());
                state.native_clipboard.sequence=written;state.native_clipboard.hash=Some(hash);state.clipboard.sent(written);state.native_clipboard.staged.push(staged);
                app.set_native_clipboard_status("Files ready on Windows clipboard; keep MacCanvas open until pasting finishes".into());
            }
        }
        Err(_)=>app.set_native_clipboard_status("Native file transfer failed or timed out; clipboard unchanged. Retry by copying again.".into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn abandoned_file_workers_keep_the_global_slot_until_exit() {
        let slot = Slot::reserve().unwrap();
        assert!(Slot::reserve().is_err());
        drop(slot);
        assert!(Slot::reserve().is_ok());
    }
}
