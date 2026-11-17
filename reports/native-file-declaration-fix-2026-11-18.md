# Native file clipboard: new-item declaration fix — 2026-11-18

## Scope and result

A specific Windows → Mac failure is reproduced against Apple's native helper on macOS 27.2.1 (26B131). The file upload and its metadata archive are not the same operation as declaring new clipboard items. This patch changes only outgoing file metadata publication and its tests. Mac → Windows file export remains unresolved; no bidirectional device acceptance is claimed.

## Evidence

Baseline: afb28847cd96df7028a90c30988c6bdcbb546748. Previous user logs show file upload/SendFiles completion but no file formats on the Mac clipboard. The previous metadata-readback probe proves only that particular upload's metadata was readable, not that Finder could paste it.

Native ScreensharingAgent SSAgent_SetPasteboard_rpc at 0x100019c38 selects helper IPC based on the declaration bit (byte 2, bit 0) of message 0x1f. With an existing helper, bit 0 clear selects IPC 101, which updates cached data used to fulfill an existing promise. Bit 0 set selects IPC 100, which declares new pasteboard items. Materialized URL flavors are accepted in that declaration path; their data need not be deferred.

The exact system SSPasteboardHelper used has SHA256 7fe6cf7370d934336985b8461130ab59c7937e7ec359553e7dddbe3387506514. ReadDataFromParentProcess is at image offset 0x38dc. A temporary developer-only fixture invoked this procedure with its pasteboard pointer redirected to a unique private pasteboard. The helper main was never called; neither the general pasteboard nor system binaries were modified. No Apple binary is included in this repository or distributed artifact.

The compressed file archive emitted by the actual Rust FileMessage/Transport encoder was used for both cases:

| Helper IPC | ACK | Private pasteboard items | Readable file URLs |
|---|---:|---:|---:|
| 101: cached data update | 2 | 0 | 0 |
| 100: declare new items | 2 | 1 | 1 |

The raw native unpacker also accepts the existing URL archive. These observations reject URL encoding as the explanation for this reproduced failure and show why an ACK alone was misleading. They do not prove the Windows remote Finder workflow passes.

## Minimal change

- src/clipboard/apple.rs: set the declaration flag only for FileMessage after the existing bounded archive compression. Text, images and rich-text encoding are unchanged.
- src/apple_hps/control.rs: assert that file metadata retains the declaration flag through the existing encrypted TCP loopback.
- Existing encrypted 0x22 file upload, authentication, session generation checks and diagnostics remain unchanged.

## Regression and validation

The new file-declaration regression was first run against the old implementation and failed (actual 0, expected 1). After the patch: 93 portable clipboard tests pass; the encrypted-control loopback passes; Windows-target native API Clippy passes; cargo fmt check passes. Windows full tests, Clippy and Release package require the new CI run and must be reported separately.

## Remaining limitations and device test

Mac → Windows: the ordinary native export path filters public.file-url; previous logs contain no recognized file formats. Unknown formats must not be interpreted as files without evidence. This patch does not change that path or replace it with SSH.

For Windows → Mac, extract the new Release artifact into a new directory, connect with native clipboard/file synchronization enabled, copy a small non-sensitive file in Explorer, then use Finder Edit → Paste Item. Test a folder only after a single file works. Record whether Paste Item becomes enabled and whether pasted file content matches. Use menu operations to avoid the separate keyboard issue. Repeat after reconnect. The metadata-only diagnostic is optional and must not be equated with actual paste success.

No real Windows → Mac Finder, reverse file transfer, or folder acceptance has been completed for this patch. No formal release or main merge is part of this change.
