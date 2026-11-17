# P2-B 自动双向文字剪贴板：方案及验收边界

基线：`ae81504571907f448e4e6d11b797fa29c89e4d51`。开发分支：`codex/auto-text-clipboard`。

## 原生协议的有限验证

核对的参考源码：[peetinc/noVNC-ARD d699293](https://github.com/peetinc/noVNC-ARD/blob/d699293635083c9e0c30c8f9480993865653772a/ard/ard-patch.js)。

该实现会在 ARD 会话初始化时发送 AutoPasteboard（0x15），通过状态通知触发 ClipboardRequest（0x0b），发送/接收 0x1f 压缩 Apple pasteboard。参考项目同时具有 High Performance 模式，但不能由此认定本项目当前 HPS 通道已经具备相同协商及消息处理能力。

MacCanvas 基线在 `src/session.rs` 显式禁止 HEVC 使用普通 VNC clipboard transport；`src/apple_hps/control.rs` 对 0x1f 未实现 pasteboard 解析/通知派发。没有对用户 Mac 的实时能力协商、协议抓包或 macOS 27.2.1 接收/发送验证。因此结论是**当前兼容性尚未证实**，不是“macOS 不支持”或“原生协议不可实现”。本轮不向生产 HPS 会话试发未经验证的消息，不修改 HPS 或视频代码。

## 采用 SSH 的实现

复用现有 `files::Client`、主机指纹绑定、认证及 Windows Hello 门禁。保持一个已认证 SSH 连接，每次同步只创建有界 exec channel，不为每次复制重新连接或认证。不安装 Mac 服务端组件。仍需启用 macOS Remote Login，并以当前 `/dev/console` 图形登录账号建立 SSH。

- `src/clipboard/sync.rs`：单任务状态机、SHA-256 内容摘要、Windows 变化序号、Mac changeCount、150ms 去抖、正常操作完成后 750ms 轮询间隔；失败退避 2/4/8/16/30 秒，后续最多每 30 秒尝试一次。不打印每次轮询或剪贴板内容。
- `src/clipboard/auto.js`：固定 JXA/AppKit helper；先验证图形登录账号；通过标准输入读取 JSON，文本绝不拼进 shell 命令。使用纯文本类型、changeCount 条件写入和读前/读后版本检查。错误仅返回固定代码。
- `src/files.rs`：在现有 SSH 工作线程内串行执行自动请求，5 秒通道打开超时、5 秒操作超时、有界输出、取消 token、独立结果事件。自动操作不清除手动文件操作的取消标志或进度。
- `src/clipboard/native.rs`：Unicode Windows 快照；在持有剪贴板锁期间核对变化序号并提交，防止迟到的 Mac 读取覆盖 Windows 新复制内容。通过会话状态和输入桌面名称拒绝锁屏/切换会话/安全桌面。
- `src/profiles.rs`：按 profile UUID 保存开关、方向和文本上限，无文本持久化。默认关闭，默认 1 MiB，界面可配置 1–1048576 UTF-8 字节。
- `src/app/files_ui/auto.rs`、`src/app/files_ui.rs`、`ui/app.slint`、`ui/files.slint`：保留 Clipboard 面板及手动收发，仅增加自动开关、方向、上限及状态。后台继续同步；手动文件操作期间暂停安排自动请求。

开启、解锁恢复、连接切换后先建立基线，**不会立即把开启前的历史剪贴板发送给另一端**，只同步其后新复制的文本。Windows 与 Mac 同时变化时，已有新的 Windows 复制优先，远程写入仍需核对 Mac 版本。内容摘要和本地写入序号防止回声循环。不把秒级时钟当成跨设备全局事件顺序。

空文本、非文本、NUL 和超过上限的文本不清空另一端。自动 SSH 与普通 VNC 共享互斥，避免两个同步器同时写剪贴板。

## 生命周期和安全边界

主连接断开、不同 Mac、开关/方向变化、Windows 锁屏或会话切换会取消当前授权代次，丢弃旧结果并重新建立基线；系统桌面状态每 250ms 检查，并在应用远程结果前再次检查。关闭面板不停止已建立的 SSH 同步。同步错误不触发 HEVC 断线或重连。

**不会后台自动重试认证或缓存额外密码。** 主连接短暂重建而 SSH 尚存时，可按已保存偏好恢复；SSH 真正断开或用户手动 Disconnect 后，需要在原面板重新 Connect securely，保留既有指纹检查及必要的 Windows Hello。自动操作失败会退避；这与自动重新认证不同。

Mac 的 NSPasteboard 不提供跨进程原子 compare-and-set。本实现把 changeCount 复查紧靠写入，但仍不能承诺另一 Mac 应用恰在检查与写入之间复制时绝无竞争；未宣称具备服务端事务语义。取消也无法撤销已经在远端完成的写入。

不识别所有密码或敏感文本。开启后属于用户授权的跨设备纯文本共享。没有剪贴板内容日志、认证信息日志或剪贴板文本落盘。

## 自动化验证与真实机器验收分开记录

已建立自动化项目：

- 核心：默认关闭、Unicode/中文/Emoji/多行、循环抑制、连续复制只保留最新、同时变化、延迟读取、条件写冲突、方向、空/二进制/NUL/超限、关闭/锁屏/断线/不同 Mac 的代次隔离、有界重试、Windows 提交失败后重试。
- SSH loopback 集成：一次认证复用、多次自动读写、版本拒绝旧写、取消排队工作、同连接手动操作、敏感标记不进入审计日志；保留现有文件传输及认证/主机指纹测试。
- Windows 私有 window station：真实 Unicode clipboard、序号条件提交/拒绝、回声抑制；不操作 CI 用户的交互剪贴板。
- macOS CI 独立 runner：真实 AppKit helper 的账号检查、Unicode、版本冲突、空/NUL/超限拒绝；runner 没有匹配图形登录账号时必须明确记录无法进行正向测试，不能绕过账号检查。

Windows CI 测试、Clippy、Release artifact 的具体结果以本轮最终交付的提交和 CI 链接为准。

**尚未完成用户 Windows ↔ Mac mini M4/macOS 27.2.1 实机验收，也未完成 30 分钟稳定性测试。** CI 上的 AppKit/Windows API 测试及 SSH loopback 不等于真实两机端到端通过。

建议实测：Release 包解压至新目录；连接 HEVC；Clipboard 面板合法连接 SSH；主动开启 Auto Sync Clipboard；通过应用菜单复制新文本。分别测试记事本 ↔ 备忘录、VS Code ↔ Mac 编辑器、浏览器 ↔ 浏览器，并覆盖中文/Emoji/代码缩进、前后台、Windows 锁屏、断线重连、不同 Mac 和开关关闭。连续 30 分钟观察是否循环覆盖、重复认证、明显 CPU 或内存增长。本轮不修改独立的键盘快捷键问题。
