//! Bounded file-worker transport; it never uses the keyboard queue's disconnect-on-full path.
use super::*;
use crate::clipboard::filecopy::{
    Connection,
    wire::{Message, failed},
};
use std::sync::atomic::AtomicU32;
pub(super) struct Route {
    pub(super) id: AtomicU32,
    pub(super) cancelled: Arc<AtomicBool>,
    metadata_only: AtomicBool,
    sender: SyncSender<Message>,
}
pub struct Port {
    control: Arc<Control>,
    sender: SyncSender<WriterCommand>,
    route: Arc<Route>,
    receiver: Receiver<Message>,
    epoch: u64,
    deadline: Instant,
    finished: bool,
    interactive: fn() -> bool,
}
pub(super) enum Receipt {
    Accepted,
    Busy,
    Rejected,
}
impl Control {
    pub(super) fn revoke_files(&self) {
        if let Ok(mut route) = self.file_route.lock()
            && let Some(route) = route.take()
        {
            route.cancelled.store(true, Ordering::Release);
        }
    }
    pub(super) fn dispatch_file(&self, message: Message) {
        if let Ok(route) = self.file_route.lock()
            && let Some(route) = route.as_ref()
            && route.id.load(Ordering::Acquire) == message.session
        {
            // A diagnostic read-back stops at the root item. Do not queue file bytes
            // that the server may already have sent before processing our Stop.
            if route.metadata_only.load(Ordering::Acquire)
                && matches!(
                    message.command,
                    crate::clipboard::filecopy::wire::DATA
                        | crate::clipboard::filecopy::wire::COMPRESSED
                )
            {
                return;
            }
            if route.sender.try_send(message).is_err() {
                route.cancelled.store(true, Ordering::Release);
            }
        }
    }
    pub(super) fn file_id(&self) -> Result<u32> {
        self.file_next
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |id| id.checked_add(1))
            .map_err(|_| failed("file session counter exhausted"))
    }
}
impl Session {
    pub fn authorize_native_files(&self, epoch: u64, enabled: bool) -> Result<()> {
        self.control.file_enabled.store(
            enabled && self.uses_hevc() && self.control.accepts_native(epoch),
            Ordering::Release,
        );
        if !enabled {
            self.control.revoke_files();
        }
        Ok(())
    }
    pub fn native_file_port(&self, epoch: u64) -> Result<Port> {
        let capability = self.control.file_capability.load(Ordering::Acquire);
        if capability & 3 != 3 || capability & 4 != 0 {
            return Err(failed(
                "Mac file-copy capability is unknown, unavailable or observe-only",
            ));
        }
        if self.control.cancelled.load(Ordering::Acquire)
            || !self.uses_hevc()
            || !self.control.accepts_native(epoch)
            || !self.control.file_enabled.load(Ordering::Acquire)
        {
            return Err(failed("file transfer is not authorized"));
        }
        let mut slot = self
            .control
            .file_route
            .lock()
            .map_err(|_| failed("file route unavailable"))?;
        if slot
            .as_ref()
            .is_some_and(|route| !route.cancelled.load(Ordering::Acquire))
        {
            return Err(failed("a file transfer is already running"));
        }
        let (sender, receiver) = mpsc::sync_channel(8);
        let route = Arc::new(Route {
            id: AtomicU32::new(self.control.file_id()?),
            cancelled: Arc::new(AtomicBool::new(false)),
            metadata_only: AtomicBool::new(false),
            sender,
        });
        *slot = Some(route.clone());
        Ok(Port {
            control: self.control.clone(),
            sender: self.sender.clone(),
            route,
            receiver,
            epoch,
            deadline: Instant::now() + Duration::from_secs(600),
            finished: false,
            interactive: crate::clipboard::native::interactive_session,
        })
    }
}
impl Port {
    /// Consumes this completed-upload port; its diagnostic route is never reused for a download.
    pub fn probe_uploaded_paths(
        mut self,
        paths: &[String],
    ) -> crate::clipboard::filecopy::UploadProbe {
        self.route.metadata_only.store(true, Ordering::Release);
        crate::clipboard::filecopy::probe_uploaded_paths(&mut self, paths)
    }
    pub fn cancellation(&self) -> Arc<AtomicBool> {
        self.route.cancelled.clone()
    }
}
impl Connection for Port {
    fn complete(&mut self) {
        self.finished = true;
    }
    fn session(&self) -> u32 {
        self.route.id.load(Ordering::Acquire)
    }
    fn rotate_session(&mut self) -> Result<()> {
        self.route
            .id
            .store(self.control.file_id()?, Ordering::Release);
        self.finished = false;
        Ok(())
    }
    fn cancelled(&self) -> bool {
        self.control.cancelled.load(Ordering::Acquire)
            || self.route.cancelled.load(Ordering::Acquire)
            || !self.control.accepts_native(self.epoch)
            || !self.control.file_enabled.load(Ordering::Acquire)
            || Instant::now() >= self.deadline
            || !(self.interactive)()
    }
    fn send(&mut self, message: Message) -> Result<()> {
        let packet = message.encode()?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if self.cancelled() {
                return Err(failed("cancelled or Windows session locked"));
            }
            if Instant::now() >= deadline {
                return Err(failed("encrypted file send timed out"));
            }
            let (receipt, incoming) = mpsc::sync_channel(1);
            match self.sender.try_send(WriterCommand::NativeFile {
                epoch: self.epoch,
                session: self.session(),
                packet: packet.clone(),
                receipt,
            }) {
                Ok(()) => loop {
                    match incoming.recv_timeout(Duration::from_millis(100)) {
                        Ok(Receipt::Accepted) => return Ok(()),
                        Ok(Receipt::Busy) => break,
                        Ok(Receipt::Rejected) => {
                            return Err(failed("file send authorization expired"));
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            return Err(failed("connection closed"));
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            if self.cancelled() || Instant::now() >= deadline {
                                return Err(failed("file send acknowledgement timed out"));
                            }
                        }
                    }
                },
                Err(TrySendError::Full(_)) => (),
                Err(TrySendError::Disconnected(_)) => return Err(failed("connection closed")),
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn receive(&mut self, timeout: Duration) -> Result<Option<Message>> {
        if self.cancelled() {
            return Err(failed("cancelled"));
        }
        match self.receiver.recv_timeout(timeout) {
            Ok(message) => Ok(Some(message)),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(_) => Err(failed("connection closed")),
        }
    }
}
impl Drop for Port {
    fn drop(&mut self) {
        // A Stop carries no clipboard data; allow it to revoke a transfer after the toggle is off.
        if !self.finished {
            let _ = self
                .sender
                .try_send(WriterCommand::NativeFileCancel(self.session()));
        }
        if let Ok(mut slot) = self.control.file_route.lock()
            && slot
                .as_ref()
                .is_some_and(|route| Arc::ptr_eq(route, &self.route))
        {
            *slot = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup(capacity: usize) -> (Port, Receiver<WriterCommand>) {
        let control = Arc::new(Control::new(false));
        control.configure_native(1, true).unwrap();
        control.file_enabled.store(true, Ordering::Release);
        let (sender, receiver) = mpsc::sync_channel(capacity);
        let (incoming, files) = mpsc::sync_channel(8);
        let route = Arc::new(Route {
            id: AtomicU32::new(42),
            cancelled: Arc::new(AtomicBool::new(false)),
            metadata_only: AtomicBool::new(false),
            sender: incoming,
        });
        *control.file_route.lock().unwrap() = Some(route.clone());
        (
            Port {
                control,
                sender,
                route,
                receiver: files,
                epoch: 1,
                deadline: Instant::now() + Duration::from_secs(2),
                finished: false,
                interactive: || true,
            },
            receiver,
        )
    }
    #[test]
    fn metadata_probe_discards_racing_file_data_without_cancelling_video_or_root_metadata() {
        let (port, _receiver) = setup(4);
        port.route.metadata_only.store(true, Ordering::Release);
        for command in [102, 103].into_iter().cycle().take(32) {
            port.control
                .dispatch_file(Message::new(command, 42, vec![0; 4]));
        }
        port.control
            .dispatch_file(Message::new(101, 42, vec![0; 104]));
        assert!(!port.cancelled());
        assert!(!port.control.cancelled.load(Ordering::Acquire));
        assert_eq!(port.receiver.try_recv().unwrap().command, 101);
        assert!(port.receiver.try_recv().is_err());
    }
    #[test]
    fn readback_after_upload_cancels_only_the_new_transfer_on_drop() {
        let (mut port, receiver) = setup(4);
        port.complete();
        port.rotate_session().unwrap();
        let id = port.session();
        assert_ne!(id, 42);
        assert!(!port.finished);
        drop(port);
        assert!(
            matches!(receiver.try_recv(), Ok(WriterCommand::NativeFileCancel(value)) if value == id)
        );
        assert!(receiver.try_recv().is_err());
    }
    #[test]
    fn completed_port_does_not_cancel_the_receipt_or_delete_finished_remote_staging() {
        let (mut port, receiver) = setup(4);
        let cancellation = port.cancellation();
        port.complete();
        drop(port);
        assert!(!cancellation.load(Ordering::Acquire));
        assert!(receiver.try_recv().is_err());
    }
    #[test]
    fn incomplete_port_requests_stop_and_old_epoch_cannot_send_file_bytes() {
        let (mut port, receiver) = setup(4);
        port.control.native_epoch.store(2, Ordering::Release);
        assert!(port.send(Message::new(102, 42, vec![1, 2, 3])).is_err());
        drop(port);
        assert!(matches!(
            receiver.try_recv(),
            Ok(WriterCommand::NativeFileCancel(42))
        ));
        assert!(receiver.try_recv().is_err());
    }
    #[test]
    fn inbound_capacity_and_wrong_transfer_id_are_isolated() {
        let (port, _receiver) = setup(4);
        port.control.dispatch_file(Message::new(102, 41, vec![]));
        assert!(port.receiver.try_recv().is_err());
        for _ in 0..9 {
            port.control.dispatch_file(Message::new(102, 42, vec![]));
        }
        assert!(port.cancelled());
        assert!(!port.control.cancelled.load(Ordering::Acquire));
    }
    #[test]
    fn file_queue_contention_does_not_close_the_primary_session() {
        let (mut port, receiver) = setup(1);
        port.sender
            .try_send(WriterCommand::NativeFileCancel(1))
            .unwrap();
        let worker = thread::spawn(move || {
            thread::sleep(Duration::from_millis(15));
            receiver.recv().unwrap();
            if let WriterCommand::NativeFile { receipt, .. } = receiver.recv().unwrap() {
                receipt.send(Receipt::Accepted).unwrap();
            }
        });
        port.send(Message::new(102, 42, vec![0; 4])).unwrap();
        assert!(!port.control.cancelled.load(Ordering::Acquire));
        port.complete();
        worker.join().unwrap();
    }
}
