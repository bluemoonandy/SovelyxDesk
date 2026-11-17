# Native Apple 文件剪贴板诊断（Release 测试包）

此包包含 Windows → Mac 新文件剪贴板声明修复、此前的 HTML UTF-8 编码修复，以及默认关闭的文件协议诊断。旧实现仅更新 Apple helper 的缓存，未声明新的文件项目；新实现对文件消息设置声明标志，私有剪贴板原生实验已复现两者差异。Windows → Mac Finder 粘贴仍需实机验证，Mac → Windows 文件导出尚未修复；本包不是双向文件复制已验收的正式发布。

## 使用

解压到独立目录，退出旧 MacCanvas。在该目录打开 PowerShell，运行 Release EXE：

```powershell
$env:MACCANVAS_NATIVE_FILE_DIAGNOSTICS = '1'
.\MacCanvas-windows-x64.exe
```

不需要 Debug EXE；不要运行包内的旧上游安装脚本，不要点击旧上游升级提示。

连接后在原有 Clipboard 面板启用 Native Apple clipboard、两个同步方向以及 Sync files and folders。无需 SSH、Remote Login 或第二次认证。

1. 新建无敏感内容的小文本文件，用 Windows 资源管理器复制，切到 Mac Finder，用编辑菜单查看能否粘贴。
2. 反向文件导出仍未修复；如需补充诊断，可在 Mac Finder 使用编辑菜单复制另一个小文件，再到 Windows 资源管理器观察结果，不能以此包预期反向已通过。
3. 保留本次 log.json，并记录两个方向的大概测试时间。初次先用单个小文件，不用文件夹、大文件或多文件。
4. 可顺便验证 Windows 浏览器 → Mac 备忘录的中文 HTML 是否恢复正常；这一项与文件诊断分开判定。

测试结束后关闭程序，删除该 PowerShell 会话中的变量即可恢复默认行为：

```powershell
Remove-Item Env:MACCANVAS_NATIVE_FILE_DIAGNOSTICS
```

环境变量在进程内只读取一次，改变后需重启 EXE。

## 新日志字段

- `clipboard.apple.formats`：只统计固定协议类型的项目和 flavor 数量。`clipboard.formats` 包含 `items`、`flavors`、`empty_flavors`、`file_urls`、`finder_nodes`、`legacy_file_lists`、`file_promises`、`plain`、`png`、`tiff`、`html`、`rtf`、`other`。未知类型只累计 other，不记录类型字符串。
- `clipboard.apple.files.probe`：上传成功后，用新的 0x22 传输编号请求其实际目标的根项目元数据，随后发送 Stop。`transfer.probe` 含 `requested_roots`、`readable_roots`、`remote_status`、`outcome`、`elapsed_ms`。
- `metadata_readable` 证明所构造路径可经当前文件通道返回匹配的根元数据，不等于数据完整性校验或 Finder 粘贴成功。
- `remote_rejected` 保留整数 `remote_status` 以供协议判读；不记录服务端错误文本。其他结果包括 `unexpected_root`、`unexpected_metadata`、`no_root_metadata`、`request_failed`、`timeout`、`cancelled`、`remote_stopped`。
- 所有元数据等待共享 5 秒期限；队列写入仍沿用现有文件通道的有界 I/O 超时。诊断可能延后文件元数据发布，默认关闭。
- 不对文件数据进行完整读回、解压或本地保存。服务端在 Stop 前已经发出的数据块会在该诊断路由丢弃，不能占满文件工作队列或关闭 HEVC 主连接。
- 不记录剪贴板内容、路径、文件名、摘要、密码或密钥。既有 `files.completed/data_transfer` 仅表示工作线程的数据阶段完成，不能视作 Finder 同步成功；启用诊断时其记录时间还包含诊断等待。

## 可选的 Mac 本机格式对照

如果服务端导出为空，需要区分 Mac 自己没有文件剪贴板，还是 Apple 导出路径过滤了文件格式。可把包内 `Inspect-Mac-Clipboard-Formats.js` 放到 Mac，然后在 Finder 用菜单复制一个无敏感内容的小测试文件，在 Mac 终端运行：

```sh
osascript -l JavaScript /实际路径/Inspect-Mac-Clipboard-Formats.js
```

脚本只读取类型及计数，不读取任何格式的内容或文件路径，也不修改剪贴板。把 JSON 数字结果与本次日志一起提供即可。它不是服务端辅助程序，不需要安装或常驻。

## 证据与限制

此前 8f9277d 日志有四次 Windows→Mac 数据上传完成、文件元数据已写入 TCP，随后剪贴板导出 archive 为空，用户确认 Finder 粘贴灰色。

本机独立实验已验证：public.file-url 作为单独 flavor，可以经 AppKit 和 Carbon PasteboardPutItemFlavor 写入独立剪贴板并被 NSURL 读取，包含 Unicode 文件名。因此暂不能靠增加另一种 URL flavor 就断言修复。

本机 Apple Agent 的普通 GetPasteboard 路径给 CopyPasteboardEntries 传入启用文件 URL 过滤的参数；这是 Mac→Windows 缺少 file URL 的强线索，尚须本次远端格式计数核对。不能把本机静态代码当作远端会话已验证，更不能以此认定所有原生文件共享都不可实现。

自动化测试与 Windows CI 不等于 Windows ↔ Mac M4 文件粘贴通过。
