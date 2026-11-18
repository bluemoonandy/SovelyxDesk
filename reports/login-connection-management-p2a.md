# P2-A 登录与连接管理测试说明

## 开发边界

独立分支 `codex/login-connection-management`，基线 `ae81504`。
原生剪贴板/文件收发分支及原工作目录的未提交改动保持独立。本分支没有合并 main，也没有创建 GitHub Release。

## 界面与使用流程

- 未连接时，设备卡片占用主工作区，展示名称、端点、登录方式、凭据保护状态和 Connect。
- Details 进入独立编辑弹窗；New 添加设备。同一 Mac 的不同账号可以保存为不同设备记录。
- 初次配置默认不保存密码。用户可明确选择快速连接或 Windows Hello，并勾选保存授权；密码在成功认证后写入 Credential Manager。
- 快速模式再次打开软件可从卡片直接连接。Hello 模式仍在每次读取密码前验证；不可用时，从 Update login/Enter different credentials 手动输入。
- 修改地址、端口或认证方式会取消使用旧密码，要求重新输入；不自动降级认证方式。
- 用户名可以保存在连接偏好中，密码不显示回填。更换保护模式需要重新输入密码并成功连接；不通过配置标记迁移旧密码。
- 连接失败保留设备；错误位于设备操作区/当前弹窗。连接成功收起设备列表。
- 键盘设置移动到 Settings → Keyboard & shortcuts，按键映射逻辑不变。
- 连接阶段只展示可观测的信息：连接、认证及桌面协商、等待首帧、已连接。现有握手接口内的认证与媒体协商合并显示，不虚构独立完成信号。

## 凭据保护与兼容

旧目标 `MacCanvas/profile/<UUID>` 保留原有 Hello 门禁，不在首次运行时迁移或删除。

新目标分别使用 `MacCanvas/login/v2/quick/<digest>`、`…/hello/<digest>`；摘要绑定设备 UUID、大小写归一的主机、端口、认证方式和精确用户名。快速记录与 Hello 记录没有相互回退。更改 JSON 中的模式不能让 Hello 记录被当作快速记录读取。

密码仅通过原有 Windows Credential Manager API 保存。快速模式授权文案明确提示同一 Windows 用户下的程序可能访问凭据，不宣称额外硬件保护。

`connections-v1.json` 是独立、版本化、大小受限、原子写入的配置旁文件，仅包含非秘密绑定、保护模式、画质、重连偏好和有界历史绑定。原 `settings.json` 的严格结构不变；其 `has_saved_credentials` 继续仅表示旧凭据。新记录的保存状态独立保存在旁文件中，只在成功保存凭据后标记，避免 SSH 面板误读取不存在的旧记录。旁文件缺失不会破坏旧记录；损坏或未知版本不会被自动覆盖，旧设备列表仍读取原配置，用户可使用不保存密码的手动连接。

选择“同时删除凭据”会删除设备的旧式记录及当前/历史新式记录；默认仅删除设备。更换账号时保留有界的历史索引，以便以后明确删除。每台设备最多保留 32 个历史绑定，超过限制安全拒绝写入。

## 修改文件

- `src/login.rs`：三种保护模式、绑定、旧凭据合法解锁/迁移接口。
- `src/login/preferences.rs`：独立配置、原子保存、大小/版本/重复记录限制。
- `src/login/tests.rs`：策略、迁移和隔离测试。
- `src/credentials.rs`：新凭据来源、受审计读写、复用 Windows Vault 原语。
- `src/session.rs`、`src/session/hevc.rs`：认证前核对绑定、按策略保存密码、发出连接阶段事件；未修改视频传输、解码、帧率或恢复算法。
- `src/app/login_ui.rs`、`src/app.rs`：设备偏好、表单与连接入口、删除管理。
- `ui/app.slint`：设备首页、独立表单、局部错误、键盘设置入口。
- `tests/windows_login.rs`：真实 Windows Vault 的临时凭据集成测试，使用随机 UUID 并清理。
- `scripts/Capture-Readme.ps1`、`.github/workflows/ci.yml`：明暗主题与 125/150/200% Slint 缩放截图检查、保存密码不显示检查、截图制品。

## 验证状态

本地执行：28 项便携测试通过（包括原有画质配置测试）；真实凭据适配器的 Windows 目标 Clippy 检查通过；Slint 1.18.1 界面编译通过，首页布局没有绑定循环警告。Mac 上的交叉检查不是 Windows 执行验证。

最终源码提交：`e4255a43788e6a831d514a0a8c42fd36b3258c2b`。

Windows 作业通过：271 次测试执行成功、0 失败；13 次忽略标记来自原有手动/硬件测试，未减少测试。真实 Windows Credential Manager 集成测试、Clippy、安装器校验、5 组主题/缩放交互检查和 Release 构建均通过。

[Windows 成功作业](https://github.com/bluemoonandy/maccanvas/actions/runs/44290995033/job/134755857338)

[最终 Release 测试包](https://github.com/bluemoonandy/maccanvas/actions/runs/44290995033/artifacts/14422861729)

macOS 附加任务未获得 Hosted Runner，平台返回 `The job was not acquired by Runner of type hosted even after multiple attempts`，任务被取消；因此整套工作流不是全绿，不能把它描述为全部 CI 通过。前一版本 `3f9573a` 的相同 macOS 检查已通过；本次 Windows 作业单独为成功。

已下载核对：主程序及音频辅助程序均为 PE AMD64，SHA256SUMS 全部匹配；主程序内嵌完整源码 SHA，没有 dirty 标记。主程序大小 30836736 字节，SHA-256：`a37283d757aabde10902c4e1f8ee1454a7df165129e8d574a467099b324adb83`。

源码干净性修正仅将 CI 截图移出源码目录，并在 Release 编译前检查工作区；没有隐藏 dirty 标记。截图/交互使用 Debug 预览构建，交付的是 Release；真实 Mac 登录和 GPU 实机验收尚未完成。

仍需真实 Windows/Mac 验收，自动化结果不能替代：

- [ ] RTX 3060 Ti 和 Intel Arc Pro：首次添加 Mac、选择快速模式、认证成功。
- [ ] 退出并重新打开客户端，点击 Connect，无 Hello/密码重复输入。
- [ ] Hello 模式成功、取消和不可用时的手动入口；无反复弹窗。
- [ ] 不保存模式退出后没有持久化密码。
- [ ] Mac account / VNC password 分别连接，后者使用兼容的标准 VNC 画质。
- [ ] 多 Mac、多账号、错误密码更新、编辑地址后不会复用旧端点密码。
- [ ] 画质切换、短线重连、手动断开，偏好与凭据策略正确。
- [ ] 默认删除仅移除设备；勾选后一起删除全部关联凭据。
- [ ] 真实 125/150/200% Windows DPI 下检查布局、焦点、文本及输入。
- [ ] HEVC、音频、剪贴板、SSH 文件功能在该稳定基线上无退化。

CI 的明暗主题、125/150/200% 交互检查通过，但截图受到运行器显示区域裁切，不能视为完整的真实 DPI 视觉验收。实际布局仍需在 Windows 显示器上确认。

## 回滚及限制

先退出应用并备份原 `settings.json` 和新 `connections-v1.json`，再切回稳定 EXE。旧设备及旧 Hello 记录未自动删除，稳定版仍能读取原设置。新添加且仅保存到 v2 的快速凭据不会自动提供给旧版；旧版需手动输入。不要删除系统 Credential Manager 中的记录来尝试回滚。

如新旁文件损坏，保留其原件，退出应用后将其改名备份，再重新配置新模式；这不会删除旧 Credential Manager 记录。不要通过编辑保护标记转换旧凭据。

首版使用 Connect 按钮和 Details 操作入口，没有双击连接设置。VNC password 使用标准 VNC；当全局配置仍为 HEVC 时，连接配置会选择 ZRLE，认证方式保持 VNC password，不会因 Mac account 认证失败而切换认证方式。没有增加对 Mac 原生剪贴板协议或 SSH 文件传输的修改。


新连接偏好在 Connect 时持久化；验证画质或自动重连偏好时，请连接一次后再退出重开。原有 Hello 记录不自动转成 v2 记录；需要新保护策略时重新输入密码并连接一次。


## 快速连接实测步骤

将 Release 测试包解压到新目录，运行 `MacCanvas-windows-x64.exe`。旧设备打开 Details，选择 Quick connect，重新输入一次 Mac 账号和密码并勾选保存授权，再连接。成功后退出并重开客户端，点击卡片 Connect，检查没有 Hello 或账号密码重复输入。

测试期间不要通过内置更新覆盖该测试版；本轮没有调整既有更新源。不要使用 Debug EXE 验收 HEVC 实际帧率。

不保存模式不会自动清除过去存在的凭据；需要移除时请明确选择同时删除凭据。自动化成功不代表 RTX 3060 Ti、Intel Arc Pro 或 Mac M4 的真实会话验收已完成。
