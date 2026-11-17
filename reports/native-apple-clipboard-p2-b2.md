# P2-B2 原生 Apple 剪贴板：协议对照与最小实验

基线 `a5e90e82159ff38876a8923af047b9e9fea512a7`；独立分支 `codex/native-apple-clipboard`。

## 证据、版本和许可证

- [iShareScreen RFC](https://github.com/renegadelink/iShareScreen/blob/9ab40d3a3151524f954cff9d6239d891197c1705/docs/apple_vnc_rfc.md)，§5.5、§6、§8.2/8.2.1、§8.12、§10；AGPL-3.0-or-later。与 MacCanvas 既有 HPS 实现所参考的版本一致。
- [open-screen-sharing clipboard.go](https://github.com/nekohasekai/open-screen-sharing/blob/1241a8337625869b912c898e72d9aa71a8301b27/internal/rfb/clipboard.go)、[messages.go](https://github.com/nekohasekai/open-screen-sharing/blob/1241a8337625869b912c898e72d9aa71a8301b27/internal/rfb/messages.go)、[status.go](https://github.com/nekohasekai/open-screen-sharing/blob/1241a8337625869b912c898e72d9aa71a8301b27/internal/rfb/status.go)、client.go、libsharing/session.go。该版本 LICENSE 为 GPL-3.0-or-later；不采用网页索引中过期的许可证标签。
- [noVNC-ARD](https://github.com/peetinc/noVNC-ARD/blob/d699293635083c9e0c30c8f9480993865653772a/ard/ard-patch.js) 和 ard-constants.js；MPL-2.0。
- `lucid-fabrics/sharelinux` 在本次核查时 GitHub 网页、API 返回 404；搜索索引显示其曾为 open-screen-sharing fork，但索引不能替代源码审核。本轮使用可取得的上游作为代码证据，不声称已经核查不可取得的 fork 差异。

独立实现 Rust 编解码及有界状态管理，未复制 Go/JavaScript 的完整协议实现或引入其依赖。引用及许可证来源同时记入 THIRD_PARTY_NOTICES.txt；原有 iShareScreen 归属保持不变。

## 协议消息矩阵

所有整数为大端；以下消息均通过已经认证的 HPS AES-CBC + 完整性校验 Records 通道，不另建连接。

| 方向/opcode | 格式、语义 | 本轮接入点 |
|---|---|---|
| C→S 0x15 | 8 字节：`15 00 00 selector 00 00 00 00`；1 开始、2 停止监视 | 用户开启实验后，经 Channel.send；禁用也经同一加密层 |
| S→C 0x14 | `type,pad,u16 body_len,body`；常见 body_len=4，后两个 body 字节为状态 | 增量读取实际长度，不把所有状态消息硬编码为8字节 |
| S→C 状态2 | pasteboard changed | 向 UI 请求 Windows 当前变化序号，然后排队 Fetch；单请求合并通知 |
| S→C 状态3 | pasteboard needs data | 记录状态；实验直接发送完整文本，不发送 promise，不自动回放旧缓存 |
| S→C 状态4 | heartbeat | 不作为失败或剪贴板变化 |
| S→C 状态17(0x11) | 用户会话变化 | 清理旧代次结果，重新开启监视和建立基线 |
| S→C 状态6/8、显示布局 | 显示唤醒/连接恢复、布局切换 | 仅重启剪贴板监视，不改变视频恢复/解码行为 |
| C→S 0x0B | `0b 00 00 00` + u32 请求标记 | 单个在途请求，携带本地变化序号，回复后再允许下次Fetch |
| 双向 0x1F | 16 字节头：`1f,pad,promise,pad,u32 echo,u32 unpacked,u32 packed` + zlib 数据 | 头和载荷跨加密记录重组；一个记录尾随下一条状态也能继续解析 |
| C→S 0x06 | 标准 RFB ClientCutText，Latin-1 | Unicode 原生实验不使用此退化路径，也不发至明文连接 |

**参考差异必须保留**：RFC 的 `cmd=11`/`12` 存在十六进制/十进制或修订歧义；Go 显式常量和 noVNC-ARD 一致将4标作 heartbeat，Go 将17标作用户会话变化，11/12是光标隐藏/显示。本实现不会因光标显隐重启剪贴板。noVNC-ARD 用0关闭监视，RFC和Go均指定2，因此采用2。

ViewerInfo：MacCanvas 当前位图与 Go 实现一致。RFC只明确服务器查询其中观察模式相关位，尚无证据要求额外的剪贴板能力位；不修改既有认证、ViewerInfo或视频 Offer。Go 既会在初始 prelude 中启用，也提供 `SetSharedClipboard` 在运行中切换。本实验选择**加密控制通道就绪后**按用户授权发送，不在明文 prelude 中试发。

Pasteboard：归档是一个或多个 item 的连续记录，每个 item 为 flavor_count，随后每种 flavor：计数长度 UTI、reserved、tag_count、计数长度 key/value 对、计数长度 primary data。UTF-8 用 `public.utf8-plain-text`；接收兼容 `public.utf16-plain-text` 的大端 UTF-16，严格校验代理项。0x1F 顶层 promise 仅作为标记保留；按 Go Text() 的语义，有实际数据的文本 flavor 仍可接收，没有 primary data 的 flavor 不应用、不自动兑现文件承诺。空归档/无文本不清空另一端。zlib 接收完整流及 Z_SYNC_FLUSH 非终结流，发送后者，不把正常缺少 zlib终结尾误判为损坏。

## 最小改动与安全界限

- `src/clipboard/apple.rs`：独立编解码、增量分帧、单请求、代次/请求标记、超时、通知合并和事件限额。
- `src/apple_hps/control.rs`：在解密之后处理0x14/0x1F，发送时分成不超过32KiB的记录，复用既有 cipher、序号和 TCP。已有非剪贴板消息解析继续使用原路径。
- `src/apple_hps/mod.rs`：仅暴露控制层剪贴板接口；不更改解码器、FPS、GPU呈现、音频或 VideoWatchdog 算法。
- `src/session.rs`、`src/session/hevc.rs`：新增专用命令和有界事件队列；原生 API 拒绝非 HEVC 连接；不增加凭据解析或认证步骤。
- `src/app/native_clipboard.rs`：本地变化序号、150ms去抖、单个最新待发文本、摘要回声抑制、条件提交、锁屏/连接/用户会话代次隔离。新 Windows 复制尚未发送时合并远端通知，先排队发送再 Fetch，避免旧读取覆盖新意图。
- Clipboard 面板仅增加实验开关、两个方向及状态。默认关闭、默认仅接收；Windows→Mac 在成功接收一次新 Mac 文本后才能主动开启。实验授权仅本应用生命周期内保留，同一 Mac 断线重连自动恢复；切换不同 Mac 需要重新授权，退出应用后默认关闭。初始快照只建立基线，随后新复制的文本才应用。
- 原 SSH 自动同步功能保留；选中原生实验时暂停其自动任务并禁用 SSH 剪贴板按钮，不自动连接 SSH、不自动回退。文件传输不作改动。普通 VNC、首次 Windows Hello 和凭据保存策略保持原有行为。

资源限制：文本1MiB、压缩载荷4MiB、归档8MiB、最多16 item/64 flavor/64 tag；过大但在已知服务器100MiB上限内的消息流式丢弃，不保留载荷。超过协议级上限、无法继续对齐、连续10秒无接收进展或单条消息累计60秒仍未完成时安全拒绝控制连接，不能盲目把载荷当下一条键鼠消息。归档损坏但帧长明确时仅拒绝该剪贴板，不错误应用文本。

Fetch 5秒超时，只进入原生实验错误状态，不尝试 SSH 或额外认证。部分资料把回复4..8字节称 reserved；允许零标记仅用于无歧义的单个请求。发生超时后，迟到的零标记不能当作新请求的回复，需要调查或重新连接，而非冒险覆盖较新剪贴板。没有任何一项超时足以证明 macOS 不支持此功能。

## 诊断和证据收集

审计事件 `clipboard.apple.requested/control/problem/apply_requested` 仅记录：操作、opcode、selector/status、请求标记、压缩/解压长度、错误原因、加密通道标记、既有连接/配置 UUID。没有剪贴板原文、摘要、密码、密钥、画面或普通按键内容。`encrypted_queued` 表示已经现有 cipher 封装入队；`tcp_written` 表示该逻辑消息已交给 TCP，`tcp_pending`/`control_tcp_drained` 区分队列仍待写与后来排空。传输阶段的 value 为逻辑消息长度；排空事件 opcode=0 是汇总标记。TCP写入或 Sent 事件均不等于 Mac pasteboard 或目标应用已经接受，必须实际粘贴验证。

对于无回应，按以下链路核查：enable selector1 → 状态2或初始Fetch → fetch标记 → 0x1F长度及实际echo → 严格解压/UTI结果 → UI本地序号是否变化 → 条件写是否成功。状态17/显示唤醒会重新开启监视。日志证据不足时继续调整协议兼容性，不把 SSH 当作终点。

## 自动化与真实设备分别验收

自动化包含：确切opcode/selector字节、Unicode/Emoji/代码缩进、UTF-16-BE、空/非文本/promise、边界分片及尾随消息、畸形归档/压缩炸弹限额、超大流式丢弃、代次隔离、通知合并、超时迟到回复、光标状态不误判；加密 TCP 回环验证同一 cipher 上的双向剪贴板及碎片化记录，避免只做自编码自解码的微测试。保留原 HEVC、键盘、音频、SSH 和 Windows剪贴板测试及 Clippy/Release流程。

**用户 Mac mini M4/macOS 27.2.1 与 Windows RTX/Arc 的实际收发尚未验证。** 模拟服务端、Windows CI、macOS Runner都不等于该实机通过；没有基于源码未实现或单次无回应宣称原生不可用。

实机顺序：退出旧版本；运行新 Release；不连接 SSH，可关闭 Mac Remote Login；以原 HEVC60方式连接（仅原有首次认证）；Clipboard 开启 Native Apple clipboard，等建立基线后，用 Mac 应用菜单复制新中文/Emoji/多行文本，到 Windows 菜单粘贴核实。收到成功状态后才勾选 Windows→Mac，再用两端应用菜单验证反向粘贴。保留同场景视频/键鼠/音频对照；随后验证后台、快速复制、Windows锁屏、Mac锁屏解锁、断线重连及30分钟运行。每项设备结果必须单独记录，当前不将这些项目标为通过。
