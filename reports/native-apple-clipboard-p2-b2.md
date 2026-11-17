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
- Clipboard 面板仅增加实验开关、两个方向及状态。默认关闭；用户开启后默认双向同步新复制的文本，两方向可独立关闭，不再要求先完成 Mac→Windows 接收验证。实验授权仅本应用生命周期内保留，同一 Mac 断线重连自动恢复；切换不同 Mac 需要重新授权，退出应用后默认关闭。初始快照只建立基线，随后新复制的文本才应用。
- 原 SSH 自动同步功能保留；选中原生实验时暂停其自动任务并禁用 SSH 剪贴板按钮，不自动连接 SSH、不自动回退。文件传输不作改动。普通 VNC、首次 Windows Hello 和凭据保存策略保持原有行为。

资源限制：文本1MiB、压缩载荷4MiB、归档8MiB、最多16 item/64 flavor/64 tag；过大但在已知服务器100MiB上限内的消息流式丢弃，不保留载荷。超过协议级上限、无法继续对齐、连续10秒无接收进展或单条消息累计60秒仍未完成时安全拒绝控制连接，不能盲目把载荷当下一条键鼠消息。归档损坏但帧长明确时仅拒绝该剪贴板，不错误应用文本。

Fetch 5秒超时，只进入原生实验错误状态，不尝试 SSH 或额外认证。部分资料把回复4..8字节称 reserved；允许零标记仅用于无歧义的单个请求。发生超时后，迟到的零标记不能当作新请求的回复，需要调查或重新连接，而非冒险覆盖较新剪贴板。没有任何一项超时足以证明 macOS 不支持此功能。

## 诊断和证据收集

审计事件 `clipboard.apple.requested/control/problem/apply_requested` 仅记录：操作、opcode、selector/status、请求标记、压缩/解压长度、错误原因、加密通道标记、既有连接/配置 UUID。没有剪贴板原文、摘要、密码、密钥、画面或普通按键内容。`encrypted_queued` 表示已经现有 cipher 封装入队；`tcp_written` 表示该逻辑消息已交给 TCP，`tcp_pending`/`control_tcp_drained` 区分队列仍待写与后来排空。传输阶段的 value 为逻辑消息长度；排空事件 opcode=0 是汇总标记。TCP写入或 Sent 事件均不等于 Mac pasteboard 或目标应用已经接受，必须实际粘贴验证。

对于无回应，按以下链路核查：enable selector1 → 状态2或初始Fetch → fetch标记 → 0x1F长度及实际echo → 严格解压/UTI结果 → UI本地序号是否变化 → 条件写是否成功。状态17/显示唤醒会重新开启监视。日志证据不足时继续调整协议兼容性，不把 SSH 当作终点。

## 自动化与真实设备分别验收

自动化包含：确切opcode/selector字节、Unicode/Emoji/代码缩进、UTF-16-BE、空/非文本/promise、边界分片及尾随消息、畸形归档/压缩炸弹限额、超大流式丢弃、代次隔离、通知合并、超时迟到回复、光标状态不误判；加密 TCP 回环验证同一 cipher 上的双向剪贴板及碎片化记录，避免只做自编码自解码的微测试。保留原 HEVC、键盘、音频、SSH 和 Windows剪贴板测试及 Clippy/Release流程。

**用户 Mac mini M4/macOS 27.2.1 与 Windows RTX/Arc 的实际收发尚未验证。** 模拟服务端、Windows CI、macOS Runner都不等于该实机通过；没有基于源码未实现或单次无回应宣称原生不可用。

实机顺序：退出旧版本；运行新 Release；不连接 SSH，可关闭 Mac Remote Login；以原 HEVC60方式连接（仅原有首次认证）；Clipboard 开启 Native Apple clipboard，等建立基线后，用 Mac 应用菜单复制新中文/Emoji/多行文本，到 Windows 菜单粘贴核实；也可先从 Windows 复制新文字到 Mac 菜单粘贴，不要求固定顺序。保留同场景视频/键鼠/音频对照；随后验证后台、快速复制、Windows锁屏、Mac锁屏解锁、断线重连及30分钟运行。每项设备结果必须单独记录，当前不将这些项目标为通过。


## 后续本地开发：取消单向验证门槛、PNG 图片（尚未打包）

用户已反馈 `be6ad68` 在其 Windows/Mac 实机上双向文字复制均通过。这属于用户的初步功能验证，未扩展为30分钟、锁屏或全部GPU场景通过的结论。

当前工作区取消必须先成功接收 Mac 文字才能发送的门槛；自动同步默认仍关闭，授权开启后默认双向，只同步新复制内容。

图片第一阶段：复用原 `0x1F` 归档，用 `public.png` flavor携带PNG；不另建连接或认证，不启用SSH。Windows读取注册的PNG格式，或常见24/32位DIB/DIBV5截图，转换成PNG；接收PNG后同时写入注册PNG和DIBV5，供不同应用选择。PNG编解码器使用锁文件原已包含的 `png 0.18.1`，仅增加直接依赖，没有升级现有包。

- 图片原始PNG上限为4MiB减4096字节，为外层zlib/归档预留空间；像素总数上限3840×2160。不是保证所有4K截图均满足字节限制。
- Windows读取、PNG完整解码、DIB转换和发送归档压缩在单个后台任务中完成；最多一个任务在运行。线程异常或切换连接后仍释放全局槽位，不无限创建转换线程。
- UI收到转换结果时重新核对授权、连接代次、本地变化序号、Windows交互会话和新的远端变化，再在剪贴板锁内比较序号后替换。分配准备在清空剪贴板之前完成。
- 忽略PNG分配缓冲区中IEND之后的尾部；不传输DIB行填充、文件路径列表或文件预览图片。PNG CRC/像素/结尾校验失败、超限、动画PNG或不支持DIB编码均拒绝，不以文本或空值覆盖另一端。
- 剪贴板日志只增加format及字节数，不记录图像、文本、摘要或文件路径。
- 此第一阶段仅支持PNG；TIFF及富文本的后续本地改动见下一节。文件与文件夹仍未接入。图片转换首次实现尚未测量对真实HEVC帧率的影响。

验证包括PNG归档及分片、双向同一加密TCP回环、PNG/DIB行方向、颜色/透明度、尾部裁剪、损坏图片/超限拒绝、后台任务并发限额和旧代次/序号拒绝。Windows私有剪贴板站的既有测试增加PNG读写和旧结果不覆盖新文本；本地仅交叉编译该Windows测试，未在Windows上运行。当前不打包、不触发CI，不将本地隔离测试当作整应用或实机图片同步验收。

参考：[Microsoft标准剪贴板格式](https://learn.microsoft.com/en-us/windows/win32/dataxchg/standard-clipboard-formats)、[png解码资源限制](https://docs.rs/png/0.18.1/png/struct.Decoder.html)。后者仅限制库内部工作集，调用端另行限制像素、输出缓冲及传输长度。


## 后续本地开发：TIFF、HTML/RTF及原生文件能力诊断（尚未打包）

| 格式 | Mac控制通道 | Windows本地 | 实现边界 |
|---|---|---|---|
| TIFF | 接收`public.tiff`，原始数据在后台转PNG | 读取`CF_TIFF`，同样转PNG；接收图片写PNG及DIBV5 | 单页8位RGB/RGBA/灰度；未压缩、PackBits、LZW、Deflate；关联alpha还原；旋转、CMYK、16位、多页、BigTIFF、JPEG/Zstd/WebP压缩暂不支持 |
| HTML | 双向`public.html` UTF-8 | 注册`HTML Format`，转换CF_HTML的字节偏移头 | 保留HTML上下文；校验范围和Unicode字符边界；不在MacCanvas渲染或执行 |
| RTF | 双向`public.rtf`原始字节 | 注册`Rich Text Format`，保留其编码控制字 | 含NUL的二进制RTF暂不支持，不自动转码 |
| 纯文本备用 | 同一item附带`public.utf8-plain-text` | 同一次Windows写入附带`CF_UNICODETEXT` | 只在原复制操作提供纯文本时附带，不通过执行HTML或猜测RTF来生成 |

富文本及图片均复用原连接及授权、序号、代次、Windows锁屏和并发隔离逻辑。格式转换、摘要及发送归档压缩在后台任务执行，Windows剪贴板读取遇到暂时占用最多尝试3次、间隔20ms。富文本的摘要包含每种格式及长度，格式变更也会同步；摘要不记入日志。

新增`tiff = 0.11.3`直接依赖，仅启用LZW/Deflate；该版本和新增MIT归属已固定。其依赖均已存在于原锁文件，无现有依赖版本升级。接收归档解压上限从8MiB增至36MiB，以容纳4K未压缩TIFF；TIFF字段上限34MiB，预检初始目录最多256项、单字段1MiB/累计元数据2MiB，图片解码输出仍限制3840×2160像素。传输压缩载荷仍限制4MiB，不能保证所有复杂4K画面都能传输。两级接收队列各只保留最新一份快照，避免堆积16份大TIFF。HTML/RTF各1MiB，超限或格式无效时不应用该富格式，已有有效纯文本可独立保留。

自动化增加：TIFF各压缩形式、RGB精确像素、关联alpha、异常目录/多种不支持格式拒绝；CF_HTML Unicode字节偏移、越界、字符截断；HTML/RTF+纯文本同item归档；旧代次发送拒绝；富文本经过同一真实加密TCP回环；Windows私有剪贴板站的富文本多格式写入和旧结果拒绝测试。本地的Windows项仅做交叉编译，未在Windows执行。Mac/Windows真实TIFF及富文本互粘、应用格式保留和性能仍待验收。

### 文件与文件夹：早期协议对照记录（当前实现见下一节）

参考同一固定版本的[协议](https://github.com/nekohasekai/open-screen-sharing/blob/1241a8337625869b912c898e72d9aa71a8301b27/internal/rfb/filecopy/protocol.go)、[能力判断](https://github.com/nekohasekai/open-screen-sharing/blob/1241a8337625869b912c898e72d9aa71a8301b27/internal/rfb/filecopy_session.go)、[握手元数据](https://github.com/nekohasekai/open-screen-sharing/blob/1241a8337625869b912c898e72d9aa71a8301b27/internal/rfb/handshake.go)。

- 文件复制是独立`0x22`消息，14字节头：opcode/pad、u32长度、u16版本、u16命令、u32传输会话。上游接收上限4MiB，发送上限1MiB、常规块64KiB。
- 命令1/2开始发送/接收，3/4/5暂停/继续/停止，100元信息、101新文件/目录、102数据、103压缩数据、104结束、200接收完成。新item包含类型、层级、名称、长度和时间等；不能把`public.file-url`直接当作文件内容。
- 上游先检查Apple扩展ServerInit中的MSB-first命令位图是否允许0x22，并拒绝observe-only会话。当前增加只读诊断，保留正常桌面名称解析，不修改认证或控制权限。
- 启用原生剪贴板时，`clipboard.apple.control`增加`opcode=34/detail=file_copy_capability`；value为位掩码：bit0能力信息已知、bit1接受0x22、bit2仅观察。未知为0，未知不等于已证实不支持。该记录不向服务器发送0x22，也不记录主机名或文件路径。
- 尚未接入原生文件传输管理器、Windows CF_HDROP/虚拟文件的交接、远端暂存与粘贴目标、进度/取消、安全目录落盘和文件夹层级；不会擅自向Mac桌面上传文件或将文件预览伪装为复制成功。现有SSH文件传输保持不变。

后续须先用实机能力诊断确认支持，再接单文件传输及取消/重名/中断清理，最后接目录树（含Windows保留名称、路径越界、链接/重解析点拒绝）。当前不宣称已支持原生文件或文件夹复制。

参考：[CF_HTML规范](https://learn.microsoft.com/en-us/windows/win32/dataxchg/html-clipboard-format)、[tiff 0.11.3功能开关](https://docs.rs/crate/tiff/0.11.3/features)。本节开发仍未提交、推送、触发CI或打包；历史`be6ad68`的通过不能替代新改动的验证。


## 文件／文件夹原生传输：本地实验实现与测试包准备

用户随后授权继续文件／文件夹开发，并要求完成剪贴板后打包测试。本节取代上一节“尚未接入传输”的状态；尚无Mac M4实机文件验收。

- `src/clipboard/filecopy/`：独立实现0x22增量分帧、文件/目录树、64KiB块发送、连续zlib接收、暂停/停止/完成处理、路径与资源限制。
- `src/session/native_files.rs`：复用现有HEVC writer/cipher的有界工作通道；检查授权、连接代次、传输编号、Windows交互会话及超时。文件队列满时退避或终止该传输，不能走键盘队列满时断开主连接的路径。每次视频轮询最多处理一个文件发送命令，让文件工作留出视频/输入处理机会；未改解码、pacer或Watchdog算法。
- `src/apple_hps/control.rs`及mod.rs：解密后分发0x22，发送仍用原Records通道。服务器能力未知、不支持或observe-only时拒绝文件传输；真实用户会话变更17会清除能力，需新握手/重连后恢复文件功能，不沿用旧用户的权限。
- `src/app/native_clipboard/files.rs`：全应用只允许一个文件工作线程；旧连接、Windows新复制、Mac新复制、关闭开关或锁屏会撤销旧任务。成功之前不发布文件剪贴板。
- `src/clipboard/apple.rs`：多item的`public.file-url`编码与解码；元数据与文件数据都通过同一加密连接。只含promise/bookmark、没有可读取file URL的Mac剪贴板暂不支持，不退回SSH。
- `src/clipboard/native.rs`：读取/写入Windows CF_HDROP，写入Preferred DropEffect=copy；不移动或删除原文件。剪贴板锁内检查原序号，旧结果不能覆盖新复制。

### 两个方向

Windows→Mac：读取实际文件/目录，上传至Mac `/tmp/maccanvas-<随机UUID>/`，请求目录0700、文件0600，待0x22接收成功后才排队发送指向完成内容的file URL。目标应用接受剪贴板仍须真实Paste验证，服务器ACK不是粘贴完成证明。不会直接写入用户桌面或替换已有目标文件。

Mac→Windows：在权限受限的Windows临时目录完整接收各根项目，校验声明长度、层级及完成标志，然后一次性发布CF_HDROP。Windows保留暂存数据直到应用退出，供资源管理器粘贴；需保持应用运行到粘贴结束。正常退出尽力清理，外部占用或崩溃可能留下缓存；Mac `/tmp`完成副本留待系统/用户清理，本实现没有秘密SSH删除任务。

### 限制与保护

- 默认关闭独立“Sync files and folders”开关。文字/图片授权不会自动开启文件读取。
- 每批最多16个根、4096个项目、64层、256MiB逻辑数据；Windows暂存缓存最多128批/1GiB；每消息最多4MiB、单次解压最多16MiB、接收队列8条、总传输10分钟/无进展30秒、发送等待10秒。声明文件未完成、超量数据和重复目标均拒绝；失败下载不发布部分目录。
- Windows接收目录设受保护DACL（owner及SYSTEM）；目录句柄不共享删除，文件create_new，不跟随重解析点；源文件读取期间不共享写入/删除，并核对大小/修改时间。拒绝路径穿越、设备名、ADS、符号链接、重解析点和大小写碰撞。Mac权限请求是否被具体服务端完整遵守须实机核查。
- 普通文件与目录数据优先；Mac resource fork、扩展属性、原始ACL、可执行权限等不保留。含链接的.app包、云占位文件、Outlook等虚拟文件剪贴板不属于已支持范围；Mac冒号/Windows保留名称会报错，不静默改名。
- 日志只记录方向、字节数、代次和完成/失败阶段；不记录路径、文件名、URL、内容或摘要。

### 验证与构建状态

本地可移植隔离测试覆盖目录二进制逐字节传输、空目录/空文件、分片及尾随控制消息、Unicode CF_HDROP/URL、遍历/保留名称/重复文件、截断与压缩炸弹、链接拒绝、取消和旧代次拒绝、通道拥塞不关闭主连接、已完成传输不误Stop、全局任务限额。加密TCP回环另外验证真实Records上的64KiB文件数据分片、回包和文件URL元数据。Windows接口及新增私有剪贴板站测试已交叉编译；运行Windows测试和完整Release构建交由CI，最终结果另行交付。

原工作区Git pack出现“stat为5488541字节但实际读取0字节”的异常，无法执行可靠的原地Git diff；没有对原库做重置/修复。源码已备份，另取GitHub固定be6ad68源码作差异复核，并在独立构建副本准备提交和CI，保留原工作区未提交文件及无关HEVC报告。

Windows权限依据：[文件ACL及继承](https://learn.microsoft.com/en-us/windows/win32/fileio/file-security-and-access-rights)、[SDDL SID字符串](https://learn.microsoft.com/en-us/windows/win32/secauthz/sid-strings)。不将Windows CI或模拟服务端通过等同于真实Mac M4的文件粘贴成功。
