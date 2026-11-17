use maccanvas::clipboard::native;
use windows::{
    Win32::{
        System::{
            StationsAndDesktops::{
                CloseDesktop, CloseWindowStation, CreateDesktopW, CreateWindowStationW,
                DESKTOP_CONTROL_FLAGS, GetProcessWindowStation, GetThreadDesktop,
                SetProcessWindowStation, SetThreadDesktop,
            },
            Threading::GetCurrentThreadId,
        },
        UI::WindowsAndMessaging::{
            CWF_CREATE_ONLY, CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WINSTA_ALL_ACCESS,
            WS_POPUP,
        },
    },
    core::{HSTRING, w},
};

#[test]
#[ignore = "Requires permission to create a private window station; CI runs this explicitly without touching an interactive user's clipboard"]
fn native_unicode_and_echo_prevention_use_a_private_station_not_the_users_clipboard() {
    unsafe {
        let previous_station = GetProcessWindowStation().unwrap();
        let previous_desktop = GetThreadDesktop(GetCurrentThreadId()).unwrap();
        // CI creates a fresh named station with naming permission; never reuse an
        // existing station or fall back to the interactive user's clipboard.
        let name = HSTRING::from(format!(
            "MacCanvas.ClipboardFixture.{}",
            uuid::Uuid::new_v4().simple()
        ));
        let station =
            CreateWindowStationW(&name, CWF_CREATE_ONLY, WINSTA_ALL_ACCESS as u32, None).unwrap();
        SetProcessWindowStation(station).unwrap();
        let desktop = CreateDesktopW(
            w!("ClipboardFixture"),
            None,
            None,
            DESKTOP_CONTROL_FLAGS(0),
            0x01ff,
            None,
        )
        .unwrap();
        SetThreadDesktop(desktop).unwrap();
        let window = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            w!("MacCanvas clipboard fixture"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let owner = window.0 as isize;
        let result = std::panic::catch_unwind(|| {
            let text = "Native caf\u{e9} \u{1f34e}\nsecond line";
            let mut watcher = native::Watcher::default();
            watcher.receive(owner, text).unwrap();
            assert_eq!(&*native::read(owner).unwrap().unwrap(), text);
            assert!(
                watcher.paste(owner).unwrap().is_none(),
                "remote clipboard writes must not echo"
            );
            native::write(owner, "new Windows text").unwrap();
            let (sequence, text) = watcher.paste(owner).unwrap().unwrap();
            assert_eq!(&*text, "new Windows text");
            watcher.sent(sequence);
            assert!(watcher.paste(owner).unwrap().is_none());
            let before = native::sequence();
            native::write(owner, "newer local copy").unwrap();
            assert!(
                native::write_if_sequence(owner, "stale remote", before)
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                native::read(owner).unwrap().unwrap().as_str(),
                "newer local copy"
            );
            let before = native::sequence();
            let applied = native::write_if_sequence(owner, "fresh remote", before)
                .unwrap()
                .unwrap();
            assert_eq!(applied, native::sequence());
            assert_eq!(
                native::read(owner).unwrap().unwrap().as_str(),
                "fresh remote"
            );
            assert!(
                !native::interactive_session(),
                "a private test desktop is not an authorized interactive desktop"
            );
            native::write(owner, "").unwrap();
            assert!(native::read(owner).unwrap().unwrap().is_empty());
            assert!(native::write(owner, "bad\0text").is_err());
            let mut dib = vec![0; 44];
            dib[..4].copy_from_slice(&40u32.to_le_bytes());
            dib[4..8].copy_from_slice(&1u32.to_le_bytes());
            dib[8..12].copy_from_slice(&1u32.to_le_bytes());
            dib[12..14].copy_from_slice(&1u16.to_le_bytes());
            dib[14..16].copy_from_slice(&24u16.to_le_bytes());
            dib[40..43].copy_from_slice(&[0, 0, 255]);
            let png = maccanvas::clipboard::image::dib_to_png(&dib).unwrap();
            let dib = maccanvas::clipboard::image::png_to_dib(&png).unwrap();
            let stale = native::sequence();
            native::write(owner, "newer copy wins over image").unwrap();
            assert!(
                native::write_image_if_sequence(owner, &png, &dib, stale)
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                &*native::read(owner).unwrap().unwrap(),
                "newer copy wins over image"
            );
            let applied = native::write_image_if_sequence(owner, &png, &dib, native::sequence())
                .unwrap()
                .unwrap();
            assert_eq!(applied, native::sequence());
            let (seq, source) = native::read_image_snapshot(owner).unwrap().unwrap();
            assert_eq!(seq, applied);
            match source {
                native::ImageSource::Png(data) => assert_eq!(*data, *png),
                native::ImageSource::Tiff(_) | native::ImageSource::Dib(_) => {
                    panic!("expected registered PNG format")
                }
            };
            let rich = maccanvas::clipboard::rich::RichText {
                plain: Some(zeroize::Zeroizing::new("中文🍎".into())),
                html: Some(zeroize::Zeroizing::new("<b>中文🍎</b>".into())),
                rtf: Some(zeroize::Zeroizing::new(b"{\\rtf1 bold}".to_vec())),
            };
            let stale = native::sequence();
            native::write(owner, "new local copy before rich reply").unwrap();
            assert!(
                native::write_rich_if_sequence(owner, &rich, stale)
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                &*native::read(owner).unwrap().unwrap(),
                "new local copy before rich reply"
            );
            let applied = native::write_rich_if_sequence(owner, &rich, native::sequence())
                .unwrap()
                .unwrap();
            let (sequence, read) = native::read_rich_snapshot(owner).unwrap().unwrap();
            assert_eq!(sequence, applied);
            assert_eq!(&**read.plain.as_ref().unwrap(), "中文🍎");
            assert!(read.html.as_ref().unwrap().contains("<b>中文🍎</b>"));
            assert_eq!(&**read.rtf.as_ref().unwrap(), b"{\\rtf1 bold}");
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("中文🍎.txt");
            std::fs::write(&path, b"fixture file bytes").unwrap();
            let stale = native::sequence();
            native::write(owner, "newer text wins over files").unwrap();
            assert!(
                native::write_files_if_sequence(owner, std::slice::from_ref(&path), stale)
                    .unwrap()
                    .is_none()
            );
            let written = native::write_files_if_sequence(
                owner,
                std::slice::from_ref(&path),
                native::sequence(),
            )
            .unwrap()
            .unwrap();
            assert!(native::owns_clipboard(owner));
            let (sequence, files) = native::read_files_snapshot(owner).unwrap().unwrap();
            assert_eq!(sequence, written);
            assert_eq!(files, vec![path]);
            let large = "\n".repeat(1024 * 1024 - 1);
            native::write(owner, &large).unwrap();
            assert_eq!(&*native::read(owner).unwrap().unwrap(), &large);
        });
        DestroyWindow(window).unwrap();
        SetProcessWindowStation(previous_station).unwrap();
        SetThreadDesktop(previous_desktop).unwrap();
        CloseDesktop(desktop).unwrap();
        CloseWindowStation(station).unwrap();
        result.unwrap();
    }
}
