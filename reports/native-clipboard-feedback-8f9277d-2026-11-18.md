# 8f9277d 原生剪贴板实机反馈与日志核对

## 范围与文件

日志：用户提供的 `log.json`，3,207,529 字节，修改时间 2026-11-18 02:44:43.265746 +08:00。JSON Lines，1155 条记录，无解析失败。逐行解析并仅提取允许的非敏感字段，没有输出或保存剪贴板原文、路径、文件名、认证字段及内容摘要。

时间戳为 UTC RFC3339、7 位小数；以下时间转为北京时间 UTC+08:00。全文件跨多个会话：2026-11-16 02:48:39.8265324 至 2026-11-18 02:44:43.2638433。包含旧版本记录和本次 8f9277d 版本，不应把所有事件都归于本次测试。

用户对照：文字正常；Windows 截图到 Mac 正常；RTF 双向正常；Windows 浏览器到 Mac 备忘录的中文 HTML 乱码；文件双向失败；Windows 到 Mac 的 Finder 编辑菜单粘贴项为灰色。Mac 截图是否已进入剪贴板尚未确定，文件夹尚未测试。

## 文件证据时间线

日志位置为原始 JSON Lines 行号。没有记录文件名或路径。

| 时间（2026-11-18） | 事件 | 日志行 |
|---|---|---|
| 02:25:37.4429149 | 扩展能力信息已知，支持原生 0x22 文件传输，非仅观察连接 | 111 |
| 02:27:59.829319 | 再次确认上述能力 | 340 |
| 02:34:15.392599 | 再次确认上述能力 | 732 |
| 02:34:36.930717 | Windows→Mac 数据上传完成，1087 字节 | 747 |
| 随后 | 排队发送文件剪贴板元数据 | 748 |
| 02:34:37.411872 | 服务端剪贴板回复：压缩 7 字节，解压后 0 字节 | 758 |
| 02:35:03.459848 | Windows→Mac 数据上传完成，1087 字节 | 777 |
| 随后 | 排队发送文件剪贴板元数据 | 778 |
| 02:35:04.386530 | 服务端剪贴板回复：压缩 7 字节，解压后 0 字节 | 786 |
| 02:36:04.467972 | Windows→Mac 数据上传完成，1087 字节 | 825 |
| 随后 | 排队发送文件剪贴板元数据 | 826 |
| 02:36:05.906142 | 服务端剪贴板回复：压缩 7 字节，解压后 0 字节 | 837 |
| 02:44:36.916747 | Windows→Mac 数据上传完成，3,176,445 字节 | 1144 |
| 随后 | 排队发送文件剪贴板元数据 | 1145 |
| 02:44:37.406253 | 服务端剪贴板回复：压缩 7 字节，解压后 0 字节 | 1153 |

四次上传完成到后续空导出回复分别约 0.481、0.927、1.438、0.490 秒。这些不是粘贴完成耗时。没有 Mac→Windows 文件完成事件。

### 已证实与边界

- 0x22 能力和上传完成已证实，不能继续笼统归因为不支持原生文件传输或上传网络失败。
- 文件元数据发送后导出的剪贴板 archive 为空；结合 Finder 粘贴灰色，文件剪贴板交接未成功。
- 空导出回复不能单独证明 Mac 系统剪贴板完全为空：服务端也可能过滤某些格式。
- 上传 ACK 不证明客户端构造的 Mac 暂存路径实际存在、可读，也不证明 Finder 接受了 public.file-url。日志缺少对应确认。
- 当前只把可解码的 public.file-url 当作文件来源。NSFilenamesPboardType、仅 promise/bookmark 的项目无法直接下载；现有日志没有格式清单，无法确定本次 Mac→Windows 失败究竟属于哪类。
- Mac 截图方向未出现可明确对应的图片应用事件，但缺少类型清单与精确操作时刻，不能据此判定图片编解码故障。

## HTML 根因与最小修改

Windows CF_HTML 正文为 UTF-8，转换为 Apple public.html 时此前原样发送，没有明确编码标记。纯文字与 RTF 是独立格式，不能用它们正常排除 HTML 编码问题。

在本机以自造中文、Emoji HTML 调用真实 AppKit NSAttributedString HTML 导入器：没有编码声明时无法保留原 Unicode；加入 UTF-8 BOM 或 UTF-8 meta 后保留正确。没有读取或写入系统剪贴板。

修改：

- src/clipboard/rich.rs：发送到 Mac 的 HTML 规范化为一个 UTF-8 BOM；不改 HTML 正文；解码按正文大小应用 1 MiB 限制，允许额外 3 字节编码签名。
- src/clipboard/apple.rs：仅 public.html 使用上述字节，纯文本与 RTF 不变；新增实际压缩后 wire archive 的编码回归。
- tests/macos_html_import.swift：真实 Cocoa 导入器回归，含中文、Emoji、多行及冲突旧 charset 情形；仅操作内存中的测试内容。

先加入 wire 回归，原实现失败；修改后本地隔离测试 87 项全部通过，主机 Clippy 通过。Swift 两项导入回归通过。新增测试不是 Windows/Mac 双机验收。

规范依据：[Microsoft CF_HTML](https://learn.microsoft.com/en-us/windows/win32/dataxchg/html-clipboard-format)。

## 有界协议调查

使用 reverse-engineer-anything 对本机系统自带 SSPasteboardHelper、ScreensharingAgent 与 screensharingd 做只读静态核对，没有启动这些组件，没有读取系统剪贴板，没有改变安全或会话设置。

本机 SSPasteboardHelper SHA256：7fe6cf7370d934336985b8461130ab59c7937e7ec359553e7dddbe3387506514。

其 UnpackScrapDataWithFilter（本机 arm64 地址 0x100000ec8）存在类型过滤回调与 promised-file-url / promised-file-content-type 的处理分支，说明文件 promise 有专门语义。ScreensharingAgent 的 CopyPasteboardEntries 存在可选 public.file-url 过滤分支。静态代码表明空导出结果可能受过滤影响，不能直接拿它证明系统剪贴板无任何项目。

这些是本机二进制的静态证据，不能代替远端 Mac M4 当前会话的运行证据。没有据此盲目添加 promise 格式、放宽校验或宣称文件问题已修复。

## 下一步最小证据需求

1. 在剪贴板接收解析处记录固定白名单格式的存在位、项目数量及各格式大小；禁止记录数据、路径、文件名、任意自定义格式字符串和内容摘要。
2. 分开记录数据上传完成、文件元数据实际写入加密通道、服务端通知、后续可识别文件项目的数量；不能把上传完成直接当成同步成功。
3. 使用受控测试文件确认实际落盘目标与 Finder 所需语义，再决定修改元数据格式还是接入 promise 流程。不能以静态源码缺失认定原生方案不可用，也不能把更多猜测直接塞进生产消息。

本轮文件／文件夹仍未通过实机验收；HTML 修复尚未获得新 Windows Release 包的实机复测。本轮未提交、推送或触发新的 CI。


## 后续定位与可控实验（同日）

已经补充显式 opt-in 的固定格式计数与上传目标元数据读回。操作及字段见 native-file-clipboard-diagnostics.md。诊断关闭时不新增文件请求。

本机 Apple Agent SHA256 为 a556f7c46a2c8d0114dcd9924d7ad8c7c4f0e144ff03771c4d80762edaebfe61。GetPasteboard RPC 在 0x100019ab4..0x100019ad0 传 x5=1 至 CopyPackedScrapDataForPasteboardByName；后者在 0x10002e174..0x10002e17c 转发为 CopyPasteboardEntries 第四参数。该函数对应分支在 0x10004a1d0 附近调用 public.file-url 比较并跳过该格式。此为本机 arm64e 静态线索，不是远端实机事件。

另一项独立剪贴板实验中，AppKit 与 Carbon 均能以单独 public.file-url flavor 读回相同 NSURL，现有文件和不存在的目标均如此。Carbon 需要先 PasteboardClear 才能取得写入所有权；未 clear 时出现 -25135，按 SDK 定义为 notPasteboardOwnerErr，修正实验后写入返回 0。本实现发送的 URL 格式在本地系统 API 层面可识别，但 Apple 会话交接仍须远端证明。

当前根因边界：上传传输本身已成功；Finder 文件交接未完成；Mac 普通导出路径可能主动过滤 file URL。不能合并成一个已确认根因。下一次证据应同时回答目标可读性与导出的实际格式。
