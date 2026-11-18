# P2-DISPLAY: Fit diagnostic candidate

Base: `38ba1b28797d627a78a7b2a953a254a2e81964de` (`codex/p2-ui-device-management`, v0.5.9). The replacement candidate is isolated on `codex/p2-display-latest`. It retains the latest device-card UI, login v2 credential support, native/automatic clipboard fixes and explicit native file transfer paths. The earlier `8109e20` candidate used obsolete baseline `be6ad68` and must not be used as the integrated test package. Do not merge main or replace a stable release before physical-device acceptance.

## Findings and scope

`src/geometry.rs::Viewport::fit` previously capped physical output scaling at 1.0. This implements native-size-or-smaller, contrary to Fit. A production-code regression fails before removing that cap and passes afterwards. Relative to the v0.5.9 base, no decoder, protocol, frame scheduling, keyboard, credentials, clipboard, audio, file transfer, or recovery changes are included.

This is a confirmed Fit defect, **not yet the confirmed cause of the reported laptop symptom**. A 3840x2160 effective source in a smaller physical viewport does not reach the old cap. If the reported source and viewport are confirmed, additional investigation is required; do not apply arbitrary compensation factors.

`refresh_layout` reads Slint canvas width/height in logical pixels and the current window scale factor. It converts to physical pixels exactly once, computes the Fit rectangle, then converts its physical dimensions back to logical units. GPU images preserve the decoded source size; the software renderer resamples to `Viewport.pixels`. The Slint Image uses `draw-*` with `image-fit: fill` and the input handlers use the very same `Viewport`. Padding rejects new pointer/wheel input; an existing drag clamps to the nearest edge so release is preserved. Integer raster dimensions may introduce less than one physical pixel of rounding per axis.

The existing canvas resize callback and video timer detect window/viewport/DPI changes without reconnecting the session. Automated tests exercise the geometry supplied during those changes, not real OS maximize/fullscreen/monitor events.

## Runtime dimensions: collection required

`display.geometry.changed` logs only when viewport, DPI, or source dimensions change:

1. `display.monitor.physical_px` and `dpi_scale`: current Windows monitor, physical pixels and ratio.
2. `display.window.physical_px`: Slint client window, physical pixels; `logical_px`: divided by the Slint DPI factor.
3. `display.dpi_scale`: Slint scale factor (ratio).
4. `display.viewport.logical_px`: actual video canvas excluding surrounding UI.
5. `display.frame.source_px`: effective decoded frame dimensions, source pixels.
6. `display.image.current_source_px`: currently installed image at the time of the layout sample; it may still be the previous image during first-frame/size transitions.
7. `display.fit.logical_per_source_px`: final X/Y logical scale (minor raster rounding possible).
8. `display.render.logical_xywh`: final Image and input rectangle, logical pixels.
9. `display.render.physical_px`: output raster width/height, physical pixels.
10. `display.fullscreen`, `display.renderer.gpu`, `display.input`: window mode, presentation path, mapping semantics.

Monitor data can be null if no underlying winit window/monitor is available. A move between equal-DPI monitors with unchanged geometry requires no new Fit calculation; monitor identity is sampled on the next geometry change. Logging does not claim physical presentation or count frames.

Logs are available through the existing About/open-logs action. On Windows, collect `display.geometry.changed` records from the application log. Record full build revision, decoder, HEVC30/60 or standard VNC, and display scale with screenshots. No credentials or screen contents are logged by this change.

## Before/after numbers (calculated fixtures, NOT measured device results)

| Effective source px | Physical viewport px | DPI | Before physical rectangle | After physical rectangle |
| --- | --- | --- | --- | --- |
| 1280x720 | 2880x1800 | 2.0 | 1280x720, offset 800,540 | 2880x1620, offset 0,90 |
| 1280x720 | 2880x1800 | 1.5 | 1280x720, offset 800,540 | 2880x1620, offset 0,90 |
| 3840x2160 | 2880x1800 | 2.0 | 2880x1620, offset 0,90 | unchanged |
| 3840x2160 | 3840x2160 | 1.0/1.5/2.0 | 3840x2160, offset 0,0 | unchanged |

For example, the first result is a Slint logical viewport 1440x900 and a logical render rectangle 0,45,1440,810 at DPI 2.0. Actual windowed/maximized canvas sizes are smaller and must be read from logs; monitor resolution is not a substitute for canvas size.

## Verification

Before the fix: 7 portable geometry tests, 5 passed and 2 failed (native-size cap and Fit matrix).
After the fix: 7 passed. The matrix checks 108 source/viewport/DPI combinations including 16:9, 16:10 and 4:3, downscaling and upscaling, padding rejection and drag clamping. Existing exact mapping tests remain intact. Dynamic geometry tests cover return to window and different physical/DPI combinations. These do not replace OS event or GPU/device tests.

Run portable production-code tests:

```sh
rustc --edition=2024 --test -D warnings scripts/test_display_geometry.rs -o /tmp/display-tests
/tmp/display-tests
```

The existing Windows workflow is dispatched with `build_release=true`: format, all-target tests, Windows clipboard/login integration, Clippy, installer checks, UI theme/scale captures, device layouts, loopback connection/fullscreen tests and a Release-only package. The macOS checks are dispatched independently. CI artifacts are candidates for physical-device testing, not accepted releases.

## Physical-device acceptance still required

- Laptop 2880x1800 at 200% and 150%: window, maximize, fullscreen and return.
- External 3840x2160 at 100%, 150%, 200% where available; move in both directions across DPI boundaries.
- HEVC30/60 on D3D11 and NVDEC where supported; standard VNC.
- Compare actual canvas/frame/render sizes and screenshots before/after. Test corners, center, clicks, dragging out and release, wheel over padding and remote cursor hotspot.
- Check actual unique-frame FPS, latency and GPU utilization against the stable version; no measured performance claim yet.

Phase 2 floating toolbar and fullscreen UX improvements remain pending after the initial Fit candidate is visually checked. Existing fullscreen controls remain available. Phase 3 custom Zoom/pan/persistence is deliberately gated on Fit acceptance. No Zoom test is claimed.

## Remote resolution feasibility (research only)

Apple documents Dynamic Resolution for a single virtual display in a High Performance connection: https://support.apple.com/en-by/guide/mac-help/mh14066/mac . Apple distinguishes a 3840x2160 pixel virtual display from 1920x1080 HiDPI: https://support.apple.com/en-lk/guide/mac-help/mchl1883115d/mac . This establishes Apple's native-client capability, not a public Windows HPS control API or compatibility with MacCanvas.

The current HPS code reads canvas/layout notices (`scaled`, `backing`) and builds streaming offers. This inspection does not establish a tested command for creating/resizing virtual displays. Do not change physical Mac display settings or add unverified protocol messages. Evaluate 1920x1080, 1920x1200, 2560x1440 and 2560x1600 only after confirming a supported session-level mechanism and its renegotiation/input semantics. A future local Zoom remains the fallback.

## Rollback

Keep the stable portable EXE/package. Exit the candidate and launch the stable EXE to roll back; this patch changes no saved display preferences or remote display configuration. Source rollback is reverting the display commit on its isolated branch. Do not reset the original dirty checkout.

## 新版整合包与凭证兼容

此前 `8109e20` 包基于旧版 UI，不能用于验收新版设备管理及登录行为。当前基线 `38ba1b2` 已包含 0.5.9 新版 UI、原生/自动剪贴板修复、发送文件到指定 Mac 路径以及从指定 Mac 路径下载文件。

新版使用 `%LOCALAPPDATA%\MacCanvas\settings.json` 与 `connections-v1.json`，通过 Windows Credential Manager 读取 `MacCanvas/login/v2/...` 凭证，并保留旧版凭证的兼容读取路径。旧包缺少这些新逻辑，因此不能据旧包界面断言凭证被删除。本显示补丁不改动凭证命名空间、配置目录或登录策略，也不要求重新保存密码。

Windows 实机仍需确认：新版设备卡片可见、原有登录偏好可见、保存凭证可以按原有保护模式使用，以及剪贴板/手动路径传输仍然正常。CI 的临时测试凭证不能替代用户原有凭证的实机检查。请保留原 EXE 和配置文件；不要删除 Credential Manager 记录。
