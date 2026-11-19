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
