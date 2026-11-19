# P3 品牌与国际化审计

## 基线与分支

以已交付并通过 Windows CI 的 Zoom 整合版 `ede8bfc62901a3b885180ea46ed0fc1e9e6052d8` 为基线，在 `codex/p3-brand-i18n` 独立开发。原工作区仍位于 `codex/native-apple-clipboard`，存在未提交修改，不切换或覆盖。远端设备管理分支 `38ba1b2`、剪贴板分支 `b29ae5b` 的功能已在当前整合基线中保留；不合并 main。

## 已确认品牌

- 英文：Sovelyx Desk；中文：Sovelyx 远程桌面。
- 发布者：Sovelyx；标语：Your Mac, within reach. / 远程连接，触手可及。
- 程序：SovelyxDesk.exe；发布包：Sovelyx-Desk-windows-x64。
- 图标：用户提供的 Sovelyx Desk PNG，保留原件，生成透明底派生资源及多尺寸 ICO。
- 官网产品地址 `https://sovelyx.com/desk` 按用户建议记录，需区分代码配置与实际网站上线。

## 审计结果与修改触点

| 区域 | 当前实现 | 修改文件 |
| --- | --- | --- |
| UI/布局 | Slint 1.18.1，Display ComboBox 被纵向布局拉伸 | ui/*.slint |
| 构建/版本资源 | build.rs 硬编码旧品牌，SVG→ICO，已有 16/24/32/48/64/128/256 尺寸 | build.rs、品牌配置、assets/brand |
| 元数据/链接 | src/lib.rs、src/links.rs、About/更新界面 | src/brand.rs、src/lib.rs、src/links.rs |
| 更新 | 已指向 bluemoonandy/maccanvas，拒绝上游资产；仍与旧文件名绑定 | src/updates.rs、统一品牌配置 |
| 安装/打包 | install.ps1、Package-Release.ps1 硬编码旧 EXE 与快捷方式名 | install.ps1、scripts/Package-Release.ps1 |
| 翻译 | 尚无 @tr、PO 或用户语言偏好；Rust 状态/错误仍为英文 | translations、src/i18n.rs、src/app/locale_ui.rs |
| 测试/截图 | 现有 UIA 按英文标签定位、窗口按旧标题定位 | scripts/Capture-Readme.ps1、UI/语言/品牌回归 |

## 兼容性边界

保留 `%LOCALAPPDATA%\MacCanvas` 数据目录，以及 settings.json、connections-v1.json、ui-window-v1.json、display-v1.json。保留 `MacCanvas/profile/{UUID}` 和 `MacCanvas/login/v2/...` 凭据命名空间及原 Windows Hello 策略。品牌化不需要迁移这些数据，因此不引入新目录或重写旧数据。语言偏好使用独立旁文件；损坏时回退并保留原件。

主要风险：UIA 测试标签随语言改变；旧快捷方式与新 EXE 的归属判断；更新资产改名后的严格 URL/哈希匹配；动态 Rust 状态在运行时切换语言后的刷新；中文长文及高 DPI 控件尺寸。分别通过稳定英文测试入口、所有权检查、更新禁用策略、统一显示翻译边界和中英文截图测试覆盖。

固定 Slint 版本 1.18.1 原生支持 `@tr()`、编译期 bundled PO 和 `select_bundled_translation`，无需升级框架。语言切换在第一个组件创建后进行，不重建连接或输入表单。

## 开源归属与交付约束

保留 LICENSE、THIRD_PARTY_NOTICES、原作者与协议来源归属，不将上游版权改写为 Sovelyx。About 提供许可证和对应源码入口，包内附版本/源码提交/构建说明。图标作为用户提供素材单独记录。未配置正式品牌更新资产前禁用应用内下载安装，保留说明与哈希校验实现。

## 实现与资源

品牌唯一来源是 `brand/brand.json`；`build.rs` 注入 Rust 常量和 Windows ProductName/FileDescription/CompanyName/OriginalFilename，Slint 窗口与 About 使用同一元数据。独立安装脚本携带生成的同一配置，`scripts/check_brand.py` 校验两者一致。构建时必须 `confirmed=true`，防止临时品牌误打包。

用户源图保留在 `assets/brand/sovelyx-desk-source.png`；透明底派生 PNG 为 1254×1254 RGBA，角点 Alpha=0，导出 16/24/32/48/64/128/256 PNG 与同尺寸 ICO。透明背景通过图像编辑工具生成，尺寸导出不重绘标识。原 LICENSE 和原作者归属不改写。

Slint 固定在已有 1.18.1：静态 UI 使用显式 `@tr("ui" => ...)` 和编译进 EXE 的 PO；Rust 状态、错误与动态说明使用同一资源目录下的统一中英文目录。动态占位内容（设备名、地址、路径）原样保留，缺失翻译回退原文。`scripts/translations.py` 校验 UI 覆盖、占位符和 PO 同步，`--extract` 可调用官方提取器辅助编辑。运行不需要外置翻译资源。

语言入口为 Settings → General → Language，支持 System Default / 简体中文 / English，保存于 `ui-language-v1.json`。简体中文区域 zh-CN/zh-SG/zh-Hans 使用中文，en-* 和其他未支持区域回退英文；zh-TW/zh-Hant 暂回退英文。手动选择优先，即时更新翻译、品牌显示名与字体；不重建 App 或会话，不设置/清空编辑字段。语言文件损坏时回退并拒绝覆盖原件。CLI `--language system|zh-CN|en` 供诊断与 UI 检查使用；普通启动恢复保存偏好。

Display 缩放下拉框固定为 36 逻辑像素高，禁止垂直拉伸；不全局放大 UI。中文字体优先 Microsoft YaHei UI，英文优先 Segoe UI。

## 更新、安装与旧配置

`updates_ready=false` 时，启动不发起版本网络查询，手动检查仅显示说明，应用内安装被拒绝；独立安装脚本的在线安装入口也拒绝下载。保留 HTTPS 来源限制、版本/资产匹配和 SHA-256 校验代码，不安装上游 EXE。独立源码仓库仍为 `bluemoonandy/maccanvas`。

便携包使用 SovelyxDesk.exe 和 SovelyxDesk-audio-windows-x64.exe。音频只调整相邻文件和发布资产名，保留旧缓存名及校验，未改变解码与播放。现有音频缓存继续复用。新的受管理安装目录/收据/快捷方式与旧程序独立；旧设备数据及凭据目录保持原样，未引入迁移，因此无需复制或重建旧密码。便携启动不会强制创建快捷方式。Windows Hello 仅替换提示文案，保留原校验和读取顺序。

## 验证与交付边界

- 本地使用实际源文件执行品牌、语言、显示几何和偏好测试及 Clippy；使用 1.18.1 Slint 编译器检查 bundled translations。
- Windows CI 执行 locked 测试、真实 Windows 剪贴板测试、Clippy、安装回滚与快捷方式归属检查、自替换预检、Release 及资源/导入检查。
- UIA 在英文和中文下检查设备、新增/编辑、已存 Hello 保护不泄露密码、错误重试、连接、画质、剪贴板、文件、音频、Display、About、设置、全屏与缩放；连接前后切换语言，核对已有输入和连接请求计数。中英文覆盖 100%、125%、150%、200% 软件缩放。截图由 Windows CI 捕获，不能替代实际 2.8K/4K 显示器视觉验收。
- 本机为 macOS，无用户那台 Windows 笔记本的实机访问。Windows Hello 人机验证、真实保存凭据、HEVC/NVDEC/D3D11 性能、跨屏 DPI、任务栏/Alt+Tab 和 Explorer 图标缓存仍需实机确认。合成会话使用标准 VNC，不伪装 HEVC 实测。
- Windows 文件属性中的 CompanyName 为 Sovelyx；未配置代码签名证书，不能据此声称 Windows 已验证发布者签名。产品网页尚未部署。

## 许可交付清单

- 保留原 `LICENSE`、`THIRD_PARTY_NOTICES.txt` 和原作者归属。
- About → Open Source Licenses 显示原文；Source code 指向构建提交。
- 发布包附 LICENSE、第三方声明、音频组件完整声明、对应提交源码 ZIP 与构建入口说明；SHA256SUMS 覆盖所有包内文件。
- 原 Slint、GPU/协议及音频组件的许可证/声明随源码保留，不改写为 Sovelyx 的上游版权。

## 回滚

退出 Sovelyx Desk 后重新使用保留的 `ede8bfc` MacCanvas 0.5.9 便携包及其音频组件。继续使用原 `%LOCALAPPDATA%\MacCanvas`，旧版本会忽略语言旁文件，无需删除/迁移设备或凭据；不要清空配置目录或 Windows Credential Manager。新版独立快捷方式如需移除，使用针对新版安装的卸载入口，不删除用户数据。不覆盖之前交付 ZIP，不合并 main。
