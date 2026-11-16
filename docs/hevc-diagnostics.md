# HEVC 30/60 FPS 诊断

不调整帧率、pacer、解码器选择或显示架构。诊断基线为提交 `1cace27`；
后续最小 readback 改进见本文末尾。P0 基线为提交
`0a137e6`；保留原有 P0 EXE，诊断版使用独立文件名，不能覆盖稳定版。

## 运行和取数

Windows CI 同时导出 `MacCanvas-diagnostics-debug-windows-x64-<sha>` 和原有
release 包。Debug EXE 用于功能诊断；**最终性能归因使用同一源码的 release
EXE**，因为 debug 构建会改变 Rust 热路径的成本。两种 EXE 都支持下面的开关。
在 PowerShell 中启动独立的进程：

```powershell
$env:MACCANVAS_HEVC_DIAGNOSTICS = '1'
.\MacCanvas-windows-x64.exe
# 诊断结束后关闭应用，再取消开关；不需要修改配置文件。
Remove-Item Env:MACCANVAS_HEVC_DIAGNOSTICS
```

只有值为 `1` 才启用。默认没有统计对象、额外逐帧时钟读取或诊断日志。
汇总事件名为 `media.hevc.diagnostics`，写入现有
`$HOME\.maccanvas\log.json` JSONL，每个实际经过的一秒窗口最多一条。
字段只包含计数、配置数字、耗时和后端名称，不采集帧内容或认证信息。
可以仅导出诊断事件，避免分享完整审计日志中的地址、文件路径等元数据：

```powershell
Get-Content "$HOME\.maccanvas\log.json" | ForEach-Object {
    try { $record = $_ | ConvertFrom-Json -ErrorAction Stop } catch { return }
    if ($record.event_name -eq 'media.hevc.diagnostics') {
        [pscustomobject]@{ timestamp = $record.timestamp; attributes = $record.attributes } |
            ConvertTo-Json -Depth 12 -Compress
    }
} | Set-Content .\hevc-60-release-arc.jsonl -Encoding utf8
```

每次测试用独立文件名并记录测试开始/结束时间；此命令会导出日志中已有的所有
诊断窗口，分析时只选本次时间范围。也可在启动前记录当前日志行数，结束后只
提取新增行。现有日志可能轮转；长时间测试需同时保留相应轮转文件。

在 Arc Pro 和 RTX 3060 Ti 上分别测试 HEVC30/HEVC60，保持相同 Mac 分辨率、
窗口大小、缩放、音频设置、网络、手动码率上限和滚动内容。每个组合重新连接，
预热 5 秒，再持续滚动 30 秒；记录 Windows 显示器刷新率及
`last_decode_backend`。先用 `auto`，之后按需用原有
`MACCANVAS_VIDEO_DECODER=software/d3d11/nvdec` 比较。不支持某种格式时以实际
回退后端为准，不把环境变量当作后端证明。再以关闭诊断的同一 release EXE
做一次对照，检查统计开销。

## 字段的准确含义

- `requested_fps`：所选 HEVC 预设；`wire_flags` 从实际发送配置取出，30/60 为
  4/5。`local_max_fps` 是保存的独立 FPS 设置，`pacer_fps` 为两者最小值。
  `proposal_bank`、`canvas_width/height` 反映当前提案与响应画布。
- `negotiated_fps=null`：当前响应解析器没有可靠的服务端 FPS 字段，不能声称
  已协商到请求值；`negotiated_fps_status` 明确说明这个限制。
- `rtp_timestamps_per_second`：选中视频 SSRC 中不同时间戳包组的到达率，使用
  有界的最近 256 个时间戳去重；倒序新时间戳计数但不参与时钟频率计算；统计在解密、读取 UDP 后发生，并非内核时间戳。
  `rtp_clock_fps` 用收到的相邻时间戳间隔计算，以 HEVC RTP 的 90 kHz 时钟为基准
  （[RFC 7798 §4.1](https://www.rfc-editor.org/rfc/rfc7798.html#section-4.1)）。
  实机日志中的 `rtp_clock_fps` 与墙钟 AU 计数不一致，Apple 流的时基假设仍需
  核对；当前不使用此推算值归因。缺失整个帧也会拉大间隔。实际阶段帧率以
  `count / window_seconds` 为准。
- `stages.complete_au`：RTP 重组器产出的完整、含图像的 access unit，尚未经过
  等待关键帧的过滤；不是 UDP 包数或 NAL 数。
- `backends.*.decoded_frames/decoded_fps`：解码调用成功输出的实际帧数。
  `decode_calls`、`decode_zero_output_calls` 单列，支持一次调用 0 帧或多帧及
  硬件失败后切软件；`last_decode_backend` 为最近一次实际调用的后端。
- `pacer_output`：pacer 放行；`session_delivery`：写入 UI 最新帧槽；
  `render_submit_unique`：不同帧号成功设置 Slint 图像；`after_render_unique`：
  已提交不同帧的 AfterRendering 回调。`render_callbacks` 包含重复刷新。
  **AfterRendering 不是显示器扫描输出或 DWM present 证明**；软件 UI 后端可能
  没有相同的渲染通知，不能用此字段代替外部显示测量。原有 UI FPS 计数未修改。
- 丢弃原因：`waiting_keyframe`、`pacer_overwrite`、`pacer_recovery_clear`、
  `session_queue_overwrite`、`render_request_overwrite`、`render_output_overwrite`、
  `render_stale`；`rtp_incomplete` 为重组器报告的不完整组数。
  `decode_error` 是失败调用数，不保证等于损失的图像数，不能与丢帧数直接相加。
  不同阶段可能跨秒，不能将一秒内各阶段计数相减后直接判为丢帧。
- `timings`：`decode_call` 包含解码器及其内部转换，`gpu_readback` 为读取 RGBA，
  `render_submit` 为 CPU 侧构造/复制/设置图像，`software_scale` 为缩放 worker，
  `slint_render_cycle` 为 BeforeRendering 至 AfterRendering 的墙钟时间。
  `complete_au_gap`、`rtp_timestamp_arrival_gap` 为接收线程观测的间隔，受调度、
  同线程解码和批处理影响。它们不提供 GPU 时间戳，也不把异步 GPU 上传耗时
  等同于 `set_frame_image` 的返回时间。
- 各事件同时给出 `count` 和按实际 `window_seconds` 归一化的 `per_second`。
  耗时有均值、样本数、`p95_upper_ms`；P95 使用固定 0.25ms 桶，约 256ms 以上
  的溢出桶使用窗口最大值作为保守上界。空窗口样本数为 0，不能把 0ms 当测量值。
- `video_receive_mbps` 为视频 UDP payload 字节（含 SRTP 开销）到达率；
  `session_receive_mbps` 为会话整体接收率，含控制/音频，不含 SSH、IP/UDP 开销。
  20 Mbps 本身不能证明存在码率或帧率限制。

## 如何收敛归因

| 实测组合 | 优先定位，不作为先验结论 |
| --- | --- |
| `pacer_fps=30`，解码输出接近 60，pacer 输出接近 30，覆盖计数增加 | 保存的本地 max_fps / pacer 设置 |
| RTP/AU 均约 30，后续阶段也约 30，decode/提交耗时低且无明显丢失 | 限制发生在完整输入之前；继续核实 Mac 送流、网络及接收调度，不能直接断言编码器限 30 |
| RTP 时间戳频率较高，完整 AU 较低且 incomplete 增加 | 网络丢包或重组；结合接收线程耗时排除本地反压 |
| 完整 AU 较高，解码输出较低，decode 耗时高/错误或回退增加 | 实际解码后端与解码路径 |
| 解码/pacer 较高，会话队列覆盖、提交或回调约 30 | UI 调度、readback、复制/缩放或 Slint 渲染阶段 |
| 不同帧提交高、AfterRendering 低、周期约 33ms | 渲染刷新/同步链路；需要显示器刷新率和外部 present 证据 |

先比较至少 20 个稳定窗口的计数与耗时，不以单秒峰值下结论。P95 不能通过平均
各秒 P95 得到整次测试 P95；跨窗口均值要按样本数加权。得到真实对比数据后，
才选择最小修复方案。诊断基线 `1cace27` 不包含上述候选优化。


## 最小 readback 改进（诊断 schema 2）

实机 Release 日志显示输入/解码/pacer 可达约 54 FPS，而图像提交约 29.6 FPS，
提交阶段约 18.8ms。因此仅将默认 OpenGL readback 路径改为：先创建 Slint
`SharedPixelBuffer`，让现有原生 readback 直接填充其字节切片。减少一份临时
RGBA Vec 的分配/初始化和一次整帧复制；不改变 D3D11 staging、纹理共享策略、
解码器、线程、队列容量或限帧。原有返回 Vec 的 API 保留供其他路径使用。

新 API 在调用原生代码前要求目标切片长度精确等于 width × height × 4，
错误长度不写入目标。Windows 自动化测试覆盖此边界；现有 NVDEC/D3D11
人工 GPU 测试增加两种 API 逐字节一致性及已知边界行像素验证。

新增三项耗时仅在默认 GPU readback 显示路径采集：

- `pixel_buffer_allocate`：创建 Slint 像素缓冲区。
- `slint_image_construct`：从已填充缓冲区构造 Slint Image。
- `slint_image_set`：设置 Slint 图像；样本包含缓存图像的重复设置，不等于不同帧数。

`render_submit` 仍覆盖整个提交阶段，可与 schema 1 比较；`gpu_readback` 在
schema 2 中不再包含旧 Vec 的分配/初始化，不能单独据其下降认定原生 GPU 读取
变快。比较相同 RTX 3060 Ti/Arc、4K、HEVC60、窗口和滚动负载，重点检查完整输入、
pacer 输出、不同帧提交率、队列覆盖、总提交耗时及新增分段耗时。改进是否有效、
能否达到 60 FPS，以新版 Release 的实机对比为准。
