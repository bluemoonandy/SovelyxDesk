//! Native file transfers. No SSH, file bytes, names, or paths are emitted to diagnostics.
#[path = "filecopy/disk.rs"]
pub mod disk;
#[path = "filecopy/dropfiles.rs"]
pub mod dropfiles;
#[path = "filecopy/urls.rs"]
pub mod urls;
#[path = "filecopy/wire.rs"]
pub mod wire;
use crate::error::Result;
pub use disk::{MAX_BYTES, MAX_ITEMS};
use std::{
    fs,
    io::Read,
    path::PathBuf,
    time::{Duration, Instant},
};
use wire::*;
use zeroize::Zeroizing;
pub trait Connection {
    fn complete(&mut self) {}
    fn session(&self) -> u32;
    fn rotate_session(&mut self) -> Result<()>;
    fn cancelled(&self) -> bool;
    fn send(&mut self, message: Message) -> Result<()>;
    fn receive(&mut self, timeout: Duration) -> Result<Option<Message>>;
}
fn next(connection: &mut impl Connection, timeout: Duration) -> Result<Message> {
    let deadline = Instant::now() + timeout;
    loop {
        if connection.cancelled() {
            return Err(failed("cancelled"));
        }
        if Instant::now() >= deadline {
            return Err(failed("transfer timed out"));
        }
        if let Some(message) = connection.receive(Duration::from_millis(100))?
            && message.session == connection.session()
        {
            return Ok(message);
        }
    }
}
pub struct StagedFiles {
    pub directory: tempfile::TempDir,
    pub paths: Vec<PathBuf>,
    pub bytes: u64,
}
pub fn upload(
    connection: &mut impl Connection,
    paths: &[PathBuf],
    root_name: &str,
    progress: impl FnMut(u64, u64),
) -> Result<Vec<String>> {
    upload_to(connection, paths, root_name, "/tmp", progress)
}
pub fn upload_to(
    connection: &mut impl Connection,
    paths: &[PathBuf],
    root_name: &str,
    destination: &str,
    mut progress: impl FnMut(u64, u64),
) -> Result<Vec<String>> {
    valid_remote_path(destination)?;
    let destination = destination.trim_end_matches('/');
    let destination = if destination.is_empty() {
        "/"
    } else {
        destination
    };
    let plan = disk::Plan::new(paths, root_name, || connection.cancelled())?;
    let session = connection.session();
    progress(0, plan.bytes);
    let result = (|| {
        connection.send(start(START_RECEIVE, session, destination)?)?;
        let mut info = vec![0; 52];
        info[8..16].copy_from_slice(&plan.bytes.to_be_bytes());
        let physical = plan
            .items
            .iter()
            .filter(|item| !item.directory)
            .map(|item| item.size.div_ceil(4096) * 4096)
            .sum::<u64>();
        info[16..24].copy_from_slice(&physical.to_be_bytes());
        let files = plan.items.iter().filter(|item| !item.directory).count() as u64;
        let folders = plan.items.len() as u64 - files;
        info[24..32].copy_from_slice(&files.to_be_bytes());
        info[40..48].copy_from_slice(&folders.to_be_bytes());
        connection.send(Message::new(INFO, session, info))?;
        let mut buffer = Zeroizing::new(vec![0; CHUNK]);
        let mut completed = 0;
        for item in &plan.items {
            if connection.cancelled() {
                return Err(failed("cancelled"));
            }
            connection.send(item.message(session))?;
            if !item.directory {
                let mut file = item.open()?;
                let mut remaining = item.size;
                while remaining != 0 {
                    let n = (remaining.min(CHUNK as u64)) as usize;
                    file.read_exact(&mut buffer[..n])
                        .map_err(|_| failed("source changed or cannot be read"))?;
                    connection.send(data(session, &buffer[..n]))?;
                    remaining -= n as u64;
                    completed += n as u64;
                    progress(completed, plan.bytes);
                    // Process remote pause/refusal without accumulating the entire source in memory.
                    if let Some(message) = connection.receive(Duration::ZERO)? {
                        match message.command {
                            PAUSE => loop {
                                let message = next(connection, Duration::from_secs(30))?;
                                if message.command == CONTINUE {
                                    break;
                                }
                                if matches!(message.command, STOP | END | DONE) {
                                    return Err(failed("Mac stopped upload"));
                                }
                            },
                            STOP | END | DONE => return Err(failed("Mac stopped upload")),
                            _ => (),
                        }
                    }
                }
            }
        }
        connection.send(end(session, 0))?;
        loop {
            let message = next(connection, Duration::from_secs(30))?;
            match message.command {
                DONE => {
                    let name = done(&message.body)?;
                    if !name.is_empty() && name != root_name {
                        return Err(failed("Mac returned an unexpected staging name"));
                    }
                    break;
                }
                STOP | END => return Err(failed("Mac refused upload")),
                _ => (),
            }
        }
        Ok(plan
            .items
            .iter()
            .filter(|item| item.level == 1)
            .map(|item| {
                format!(
                    "{}/{root_name}/{}",
                    destination.trim_end_matches('/'),
                    item.name
                )
            })
            .collect())
    })();
    if result.is_ok() {
        connection.complete();
    }
    if result.is_err() {
        let _ = connection.send(Message::new(STOP, connection.session(), vec![]));
    }
    result
}
/// Optional read-back of uploaded roots: stop at metadata, never decode or save file data.
#[derive(Debug, serde::Serialize)]
pub struct UploadProbe {
    pub requested_roots: usize,
    pub readable_roots: usize,
    pub remote_status: Option<u32>,
    pub outcome: &'static str,
    pub elapsed_ms: u128,
}
pub fn probe_uploaded_paths(connection: &mut impl Connection, paths: &[String]) -> UploadProbe {
    let began = Instant::now();
    let deadline = began + Duration::from_secs(5);
    let mut report = UploadProbe {
        requested_roots: paths.len(),
        readable_roots: 0,
        remote_status: None,
        outcome: "invalid_request",
        elapsed_ms: 0,
    };
    if paths.is_empty() || paths.len() > 16 {
        return report;
    }
    for path in paths {
        let mut probe_session = None;
        let result = (|| -> Result<bool> {
            if connection.cancelled() {
                return Err(failed("cancelled"));
            }
            if Instant::now() >= deadline {
                return Err(failed("probe deadline"));
            }
            connection.rotate_session()?;
            let session = connection.session();
            probe_session = Some(session);
            connection.send(start(START_SEND, session, path)?)?;
            // All metadata waits share one deadline; sends retain the existing port I/O bound.
            let mut messages = 0;
            loop {
                if Instant::now() >= deadline {
                    return Err(failed("probe deadline"));
                }
                let message = next(
                    connection,
                    deadline.saturating_duration_since(Instant::now()),
                )?;
                messages += 1;
                if messages > 8 {
                    report.outcome = "unexpected_metadata";
                    return Ok(false);
                }
                match message.command {
                    INFO if message.body.len() >= 52 => (),
                    ITEM => {
                        let body = &message.body;
                        let valid =
                            body.len() >= 105 && matches!(body[0], 1 | 2) && body[92..94] == [0, 0];
                        if !valid {
                            report.outcome = "unexpected_metadata";
                            return Ok(false);
                        }
                        let length = u16::from_be_bytes([body[100], body[101]]) as usize;
                        let expected = path.rsplit('/').next().unwrap_or("").as_bytes();
                        if length == 0
                            || body.get(104..104 + length) != Some(expected)
                            || body.get(104 + length) != Some(&0)
                        {
                            report.outcome = "unexpected_root";
                            return Ok(false);
                        }
                        return Ok(true);
                    }
                    END if message.body.len() >= 4 => {
                        let status = u32::from_le_bytes(message.body[..4].try_into().unwrap());
                        report.remote_status = Some(status);
                        report.outcome = if status == 0 {
                            "no_root_metadata"
                        } else {
                            "remote_rejected"
                        };
                        return Ok(false);
                    }
                    STOP => {
                        report.outcome = "remote_stopped";
                        return Ok(false);
                    }
                    _ => {
                        report.outcome = "unexpected_metadata";
                        return Ok(false);
                    }
                }
            }
        })();
        // A distinct transfer ID prevents this Stop from cancelling the completed upload.
        // Also stop after errors; no full read-back or temporary local file is required.
        if let Some(session) = probe_session
            && connection.send(Message::new(STOP, session, vec![])).is_ok()
        {
            connection.complete();
        }
        match result {
            Ok(true) => report.readable_roots += 1,
            Ok(false) => break,
            Err(_) => {
                report.outcome = if connection.cancelled() {
                    "cancelled"
                } else if Instant::now() >= deadline {
                    "timeout"
                } else {
                    "request_failed"
                };
                break;
            }
        }
    }
    if report.readable_roots == report.requested_roots {
        report.outcome = "metadata_readable";
    }
    report.elapsed_ms = began.elapsed().as_millis();
    report
}
pub fn download(
    connection: &mut impl Connection,
    paths: &[String],
    progress: impl FnMut(u64, u64),
) -> Result<StagedFiles> {
    download_in(connection, paths, None, progress)
}
pub fn valid_remote_path(path: &str) -> Result<()> {
    if !path.starts_with('/')
        || path.len() > 4096
        || path.chars().any(char::is_control)
        || path.split('/').any(|part| matches!(part, "." | ".."))
    {
        return Err(failed("Enter an absolute Mac path without dot components"));
    }
    Ok(())
}
pub fn download_in(
    connection: &mut impl Connection,
    paths: &[String],
    parent: Option<&std::path::Path>,
    mut progress: impl FnMut(u64, u64),
) -> Result<StagedFiles> {
    for path in paths {
        valid_remote_path(path)?;
    }
    if paths.is_empty() || paths.len() > 16 {
        return Err(failed("select between 1 and 16 Mac roots"));
    }
    let mut builder = tempfile::Builder::new();
    builder.prefix("maccanvas-native-files-");
    let directory = match parent {
        Some(parent) => builder.tempdir_in(parent),
        None => builder.tempdir(),
    }
    .map_err(|_| failed("cannot create private staging directory"))?;
    disk::protect_staging(directory.path())?;
    let mut roots = Vec::new();
    let mut completed = 0u64;
    let mut total_items = 0;
    let result = (|| {
        for (index, path) in paths.iter().enumerate() {
            if index != 0 {
                connection.rotate_session()?;
            }
            let session = connection.session();
            let root = directory.path().join(index.to_string());
            fs::create_dir(&root).map_err(|_| failed("cannot create staging subdirectory"))?;
            let mut receiver = disk::Receiver::new(root, MAX_BYTES - completed)?;
            let mut inflater = flate2::Decompress::new(true);
            let mut announced = 0;
            connection.send(start(START_SEND, session, path)?)?;
            loop {
                let message = next(connection, Duration::from_secs(30))?;
                match message.command {
                    INFO => {
                        if message.body.len() < 52 {
                            return Err(failed("truncated file totals"));
                        }
                        announced = u64::from_be_bytes(message.body[8..16].try_into().unwrap());
                        let files = u64::from_be_bytes(message.body[24..32].try_into().unwrap());
                        let folders = u64::from_be_bytes(message.body[40..48].try_into().unwrap());
                        if announced > MAX_BYTES - completed
                            || files
                                .checked_add(folders)
                                .is_none_or(|n| n > MAX_ITEMS as u64)
                        {
                            return Err(failed("remote batch exceeds limits"));
                        }
                    }
                    ITEM => {
                        receiver.item(&message.body)?;
                        if total_items + receiver.items > MAX_ITEMS {
                            return Err(failed("too many downloaded items"));
                        }
                    }
                    DATA => {
                        receiver.data(data_body(&message.body)?)?;
                    }
                    COMPRESSED => {
                        if message.body.len() < 10 || message.body[..2] != [0, 1] {
                            return Err(failed("invalid compressed file data"));
                        }
                        let size =
                            u32::from_be_bytes(message.body[2..6].try_into().unwrap()) as usize;
                        let length =
                            u32::from_be_bytes(message.body[6..10].try_into().unwrap()) as usize;
                        if size > 16 * 1024 * 1024 || size as u64 > receiver.outstanding() {
                            return Err(failed("compressed file data expands beyond limit"));
                        }
                        let compressed = message
                            .body
                            .get(10..10 + length)
                            .ok_or_else(|| failed("truncated compressed file data"))?;
                        let mut output = Zeroizing::new(vec![0; size + 1]);
                        let before_out = inflater.total_out();
                        let before_in = inflater.total_in();
                        let status = inflater
                            .decompress(compressed, &mut output, flate2::FlushDecompress::Sync)
                            .map_err(|_| failed("invalid compressed file stream"))?;
                        if inflater.total_out() - before_out != size as u64
                            || inflater.total_in() - before_in != compressed.len() as u64
                        {
                            return Err(failed("compressed file size mismatch"));
                        }
                        receiver.data(&output[..size])?;
                        if status == flate2::Status::StreamEnd {
                            inflater = flate2::Decompress::new(true);
                        }
                    }
                    END => {
                        if message.body.len() < 4 || message.body[..4] != [0; 4] {
                            return Err(failed("Mac reported a failed download"));
                        }
                        let path = receiver.finish()?;
                        roots.push(path);
                        completed += receiver.bytes;
                        total_items += receiver.items;
                        progress(completed, completed);
                        break;
                    }
                    STOP => return Err(failed("Mac stopped download")),
                    _ => return Err(failed("unexpected download command")),
                }
                progress(
                    completed + receiver.received,
                    completed.saturating_add(announced.max(receiver.received)),
                );
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = connection.send(Message::new(STOP, connection.session(), vec![]));
        return Err(error);
    }
    connection.complete();
    Ok(StagedFiles {
        directory,
        paths: roots,
        bytes: completed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    struct Fake {
        id: u32,
        inbox: VecDeque<Message>,
        sent: Vec<Message>,
        cancel: bool,
        finished: bool,
    }
    impl Fake {
        fn new(inbox: Vec<Message>) -> Self {
            Self {
                id: 42,
                inbox: inbox.into(),
                sent: vec![],
                cancel: false,
                finished: false,
            }
        }
    }
    impl Connection for Fake {
        fn session(&self) -> u32 {
            self.id
        }
        fn rotate_session(&mut self) -> Result<()> {
            self.id += 1;
            Ok(())
        }
        fn cancelled(&self) -> bool {
            self.cancel
        }
        fn complete(&mut self) {
            self.finished = true;
        }
        fn send(&mut self, message: Message) -> Result<()> {
            if message.command == END {
                self.inbox
                    .push_back(Message::new(DONE, self.id, vec![0; 4]));
            }
            self.sent.push(message);
            Ok(())
        }
        fn receive(&mut self, timeout: Duration) -> Result<Option<Message>> {
            if timeout.is_zero() {
                Ok(None)
            } else {
                Ok(self.inbox.pop_front())
            }
        }
    }
    fn item(name: &str, level: u16, directory: bool, size: u64) -> Message {
        disk::Item {
            name: name.into(),
            level,
            directory,
            size,
            source: PathBuf::new(),
            modified: None,
        }
        .message(42)
    }
    #[test]
    fn upload_probe_uses_new_session_stops_at_metadata_and_preserves_file_data() {
        let mut root = item("fixture.txt", 0, false, 5);
        root.session = 43;
        let data = Message::new(DATA, 43, vec![0; 9]);
        let mut connection = Fake::new(vec![root, data]);
        let report = probe_uploaded_paths(&mut connection, &["/tmp/fixture.txt".into()]);
        assert_eq!(report.outcome, "metadata_readable");
        assert_eq!(report.readable_roots, 1);
        assert_eq!(connection.inbox.len(), 1); // File bytes are not consumed or copied.
        assert_eq!(
            connection
                .sent
                .iter()
                .map(|x| x.command)
                .collect::<Vec<_>>(),
            [START_SEND, STOP]
        );
        assert!(connection.sent.iter().all(|x| x.session == 43));
    }
    #[test]
    fn upload_probe_distinguishes_remote_rejection_and_wrong_root() {
        let mut connection = Fake::new(vec![end(43, 2)]);
        let report = probe_uploaded_paths(&mut connection, &["/tmp/missing.txt".into()]);
        assert_eq!(report.outcome, "remote_rejected");
        assert_eq!(report.remote_status, Some(2));
        let safe = serde_json::to_string(&report).unwrap();
        assert!(!safe.contains("missing.txt"));
        assert!(!safe.contains("/tmp"));
        let mut root = item("wrong.txt", 0, false, 0);
        root.session = 43;
        let mut connection = Fake::new(vec![root]);
        assert_eq!(
            probe_uploaded_paths(&mut connection, &["/tmp/fixture.txt".into()]).outcome,
            "unexpected_root"
        );
        let mut connection = Fake::new(vec![]);
        connection.cancel = true;
        assert_eq!(
            probe_uploaded_paths(&mut connection, &["/tmp/fixture.txt".into()]).outcome,
            "cancelled"
        );
        assert!(
            connection.sent.is_empty(),
            "cancellation must not Stop the completed upload session"
        );
    }
    #[test]
    fn directory_upload_is_chunked_and_committed_only_after_receiver_done() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("中文📁");
        fs::create_dir(&source).unwrap();
        fs::create_dir(source.join("empty")).unwrap();
        let content = (0..CHUNK * 3 + 17)
            .map(|i| (i % 251) as u8)
            .collect::<Vec<_>>();
        fs::write(source.join("code.bin"), &content).unwrap();
        let mut connection = Fake::new(vec![]);
        let paths = upload(&mut connection, &[source], "maccanvas-fixture", |_, _| {}).unwrap();
        assert_eq!(paths, vec!["/tmp/maccanvas-fixture/中文📁"]);
        assert!(connection.finished);
        let target = tempfile::tempdir().unwrap();
        let mut receiver = disk::Receiver::new(target.path().to_owned(), MAX_BYTES).unwrap();
        let mut chunks = 0;
        for message in &connection.sent {
            match message.command {
                ITEM => receiver.item(&message.body).unwrap(),
                DATA => {
                    let bytes = data_body(&message.body).unwrap();
                    assert!(bytes.len() <= CHUNK);
                    chunks += 1;
                    receiver.data(bytes).unwrap();
                }
                _ => (),
            }
        }
        let root = receiver.finish().unwrap();
        assert_eq!(fs::read(root.join("中文📁/code.bin")).unwrap(), content);
        assert!(root.join("中文📁/empty").is_dir());
        assert_eq!(chunks, 4);
    }
    #[test]
    fn manual_upload_uses_the_selected_mac_directory_without_clipboard_metadata() {
        let source = tempfile::tempdir().unwrap();
        let file = source.path().join("fixture.txt");
        fs::write(&file, b"selected destination").unwrap();
        let mut connection = Fake::new(vec![]);
        let paths = upload_to(
            &mut connection,
            &[file],
            "transfer",
            "/Users/fixture/Downloads/",
            |_, _| {},
        )
        .unwrap();
        assert!(connection.finished);
        assert_eq!(paths, ["/Users/fixture/Downloads/transfer/fixture.txt"]);
        assert_eq!(
            connection.sent[0].body,
            start(START_RECEIVE, 42, "/Users/fixture/Downloads")
                .unwrap()
                .body
        );
        assert!(connection.sent.iter().all(|m| m.command != START_SEND));
    }
    #[test]
    fn manual_download_stages_in_the_selected_folder_and_never_replaces_existing_data() {
        let destination = tempfile::tempdir().unwrap();
        fs::write(destination.path().join("fixture.txt"), b"preserve").unwrap();
        let mut connection = Fake::new(vec![
            item("fixture.txt", 0, false, 5),
            data(42, b"hello"),
            end(42, 0),
        ]);
        let result = download_in(
            &mut connection,
            &["/Users/fixture/fixture.txt".into()],
            Some(destination.path()),
            |_, _| {},
        )
        .unwrap();
        assert!(result.directory.path().starts_with(destination.path()));
        assert_eq!(fs::read(&result.paths[0]).unwrap(), b"hello");
        assert_eq!(
            fs::read(destination.path().join("fixture.txt")).unwrap(),
            b"preserve"
        );
        let folder = result.directory.path().to_owned();
        drop(result);
        assert!(!folder.exists()); // A revoked or incomplete UI receipt leaves no published files.
    }
    #[test]
    fn manual_paths_are_validated_before_sending() {
        for path in [
            "relative",
            "~/Downloads",
            "/Users/fixture/../other",
            "/tmp/evil\0path",
        ] {
            let mut connection = Fake::new(vec![]);
            assert!(download_in(&mut connection, &[path.into()], None, |_, _| {}).is_err());
            assert!(connection.sent.is_empty());
        }
    }
    #[test]
    fn folder_download_publishes_only_a_complete_tree() {
        let messages = vec![
            item("folder", 0, true, 0),
            item("子目录", 1, true, 0),
            item("a.txt", 2, false, 5),
            data(42, b"hello"),
            item("empty.txt", 1, false, 0),
            end(42, 0),
        ];
        let mut connection = Fake::new(messages);
        let result = download(
            &mut connection,
            &["/Users/fixture/folder".into()],
            |_, _| {},
        )
        .unwrap();
        assert!(connection.finished);
        assert_eq!(result.bytes, 5);
        assert_eq!(
            fs::read(result.paths[0].join("子目录/a.txt")).unwrap(),
            b"hello"
        );
        assert!(result.paths[0].join("empty.txt").is_file());
    }
    #[test]
    fn traversal_truncation_and_compression_bombs_never_publish_files() {
        for messages in [
            vec![item("../escape", 0, false, 0), end(42, 0)],
            vec![item("short.txt", 0, false, 4), data(42, b"hi"), end(42, 0)],
            vec![
                item("big.bin", 0, false, 2),
                Message::new(COMPRESSED, 42, vec![0, 1, 1, 0, 0, 1, 0, 0, 0, 0]),
            ],
        ] {
            let mut connection = Fake::new(messages);
            assert!(download(&mut connection, &["/fixture".into()], |_, _| {}).is_err());
            assert!(!connection.finished);
            assert!(
                connection
                    .sent
                    .iter()
                    .any(|message| message.command == STOP)
            );
        }
    }
    #[test]
    fn cancellation_does_not_start_source_reading() {
        let mut connection = Fake::new(vec![]);
        connection.cancel = true;
        assert!(
            upload(
                &mut connection,
                &[PathBuf::from("/nonexistent")],
                "root",
                |_, _| {}
            )
            .is_err()
        );
        assert!(connection.sent.is_empty());
    }
    #[test]
    fn unsafe_windows_names_and_existing_destinations_are_rejected() {
        for name in [
            "..", "a/b", "a\\b", "C:stream", "CON.txt", "CON .txt", "lpt1", "a.", "a ", "nul",
            "x\0y",
        ] {
            assert!(disk::component(name).is_err(), "{name:?}");
        }
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("existing"), b"preserve").unwrap();
        let mut receiver = disk::Receiver::new(dir.path().into(), MAX_BYTES).unwrap();
        assert!(receiver.item(&item("existing", 0, false, 0).body).is_err());
        assert_eq!(fs::read(dir.path().join("existing")).unwrap(), b"preserve");
    }
    #[cfg(unix)]
    #[test]
    fn source_symlinks_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("real"), b"source").unwrap();
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("link")).unwrap();
        assert!(disk::Plan::new(&[dir.path().join("link")], "root", || false).is_err());
    }
    #[test]
    fn file_frames_reassemble_every_split_and_preserve_following_control() {
        let bytes = data(42, b"abc").encode().unwrap();
        for split in 0..bytes.len() {
            let mut framer = Framer::default();
            let (_, first) = framer.feed(&bytes[..split]).unwrap();
            assert!(first.is_none());
            let mut rest = bytes[split..].to_vec();
            rest.extend_from_slice(&[0x14, 0, 0, 4, 0, 1, 0, 4]);
            let (used, message) = framer.feed(&rest).unwrap();
            assert_eq!(&rest[used..], &[0x14, 0, 0, 4, 0, 1, 0, 4]);
            let message = message.unwrap();
            assert_eq!(message.session, 42);
            assert_eq!(data_body(&message.body).unwrap(), b"abc");
        }
        assert!(
            Framer::default()
                .feed(&[0x22, 0, 0xff, 0xff, 0xff, 0xff])
                .is_err()
        );
        assert_eq!(&*end(42, 0x01020304).body, &[4, 3, 2, 1]);
    }
    #[test]
    fn unicode_dropfiles_and_local_file_urls_roundtrip_without_shell_interpretation() {
        let paths = vec![
            "C:\\fixture\\中文🍎.txt".into(),
            "C:\\fixture\\a b.txt".into(),
        ];
        let bytes = dropfiles::encode(&paths).unwrap();
        assert_eq!(dropfiles::decode(&bytes).unwrap(), paths);
        let path = "/Users/fixture/中文 % #.txt";
        let uri = urls::encode(path).unwrap();
        assert_eq!(urls::decode(uri.as_bytes()).unwrap(), path);
        for uri in [
            "file://other-host/a",
            "https://example.com/a",
            "file:///tmp/%00x",
            "file:///tmp/a?query",
        ] {
            assert!(urls::decode(uri.as_bytes()).is_err());
        }
    }
}
