# P1-A4 实现与测试说明

## 基线与边界

开始任务时本地 HEAD 就是 e903d2e242d282f4689c24d51ce996502e2cf8f3，与该基线没有额外代码差异。A3 原始报告保持原样。A、B 分别提交；RTP 只调查，不调整解码行为。没有改动键盘映射、SSH 剪贴板、Windows Hello、UI 布局、裁剪或像素缓冲区算法。

## A：固定恢复观察期

- 解码成功后进入 `stabilizing`，停止当前网络恢复动作，保留恢复 episode、最初请求时间和次数。
- 从本轮观察的第一帧解码起固定观察 5 秒。连续解码不会滑动延长观察期；单帧静态桌面也不需要持续 FPS。
- 若观察期内再次出现符合现有条件的明确恢复需求，沿用原 episode、原 10/30 秒时间基准和已用尝试次数，不重新获得完整 30 秒。
- 本轮恢复已解码但还没有新提交时，后续操作不会把呈现问题升级为网络重连；观察结束后仍保留呈现等待标志，直至观察到新提交。正常后台未提交的旧帧不会阻止首次明确恢复探测。
- 唯一画面提交独立观察。提交时间必须不早于观察期的首解码时间；旧提交不能充当新的 UI 进展。该时间判断不等同于像素内容或逐帧同一性校验。
- 5 秒结束：观察到新提交则记录完成；没有提交则记录 `observation_expired_without_submission`，退出网络恢复观察，把呈现问题独立报告，不靠重连网络修复 UI。
- 纯空闲、鼠标移动仍不触发恢复。原有 TCP 断开、认证拒绝、手动断开、重连退避及最多 3 次连续自动重试路径不变。

该修复针对 A3 中两个请求间约 3.494 秒的重新开窗开销，不声称直接消除全部 42 秒。观察期为可测的策略参数，是否适合真实 Mac 熄屏恢复仍需实机数据。

### A 的日志字段

`media.hevc.resume.state` 新增：

- `media.resume.episode`：当前连接内的恢复过程编号，不是认证标识。
- `media.resume.presentation_pending`：本轮恢复的已解码帧仍等待 UI 提交，阻止用网络重连替代呈现故障处理。
- `media.resume.attempts`：同一恢复过程实际启动恢复请求的次数，重复活动不会无限增加或改起点。
- `media.resume.first_decoded_age_ms`、`media.resume.first_submitted_age_ms`：分别表示解码与提交进展，缺失为 null。
- `media.resume.observation_remaining_ms`：当前固定观察期剩余时间；不处于观察期时为 null。
- `media.resume_request_age_ms` 保持相对最初请求时间。

`media.hevc.resume.observation`：记录 `presentation_progress`、`observation_completed` 或 `observation_expired_without_submission`，附 episode、尝试次数、原请求年龄以及首/末解码和首次提交年龄。均为时间和计数，不含按键、像素或剪贴板内容。

## B：连接准备计时

在一次连接内缓存各阶段跨度，在 Offer 已调用发送后统一记录 `media.hevc.preparation.summary`，避免在每个测量阶段额外同步写日志。

`media.preparation.phases` 数组包含：

1. `control_bootstrap`：控制协议初始化。
2. `control_ready_audit_wait`：原有 control.ready 同步审计调用，包括其确认等待。
3. `session_state_prepare`：会话设置检查。
4. `audio_component_verify`：原有可选组件检查/文件验证及原有降级处理。
5. `offer_build`：协议能力/Offer 构建。
6. `offer_serialize`：HEVC 配置序列化。
7. `offer_requested_audit_wait`：原有 Offer 请求同步审计调用。
8. `set_encodings_send`：原有编码协商消息发送。
9. `offer_send`：原有 Offer 加密发送调用。

每项含 `start_utc`、`end_utc`、`start_offset_ns`、`end_offset_ns`、`duration_ns`、固定 phase 名、outcome 和安全 error_type。duration/offset 使用单调时钟；UTC 用于与其他日志对齐，不记录操作返回值或错误消息。

摘要还包含 `media.preparation.origin_utc`、总耗时、`control.output_queue_bytes`、`control.offer_socket_write_complete`。队列为 0 只说明已交给 socket，不代表 Mac 收到或视频恢复；若最初有待发送字节，后续首次观察到队列清空时记录 `media.hevc.preparation.offer_flushed`，其时间为完成被观测到的上界。

当前区间没有额外显式异步任务或业务锁。可独立观察的审计确认等待已经单列；其内部文件锁/fsync 不单独归因。没有删除认证、文件验证、审计等待、2 秒 Offer 重发或 12 秒画布等待。初始消息发送位置为暴露计时边界而移到调用方，顺序仍为 encodings→Offer，已有加密模拟服务端会验证完整协商。

旧日志没有这些分段数据，无法证明可消除的 5.702 秒具体来自哪一段。因此本轮 B 只交付诊断，不宣称已经缩短该段耗时。

## C：RTP 调查

详见同目录 `HEVC-P1-A4-RTP-investigation-2026-11-17.md`，本轮未改变组装/完整性校验及解码器重置策略。

## 验收状态与实机步骤

本文件描述实现与诊断契约，不代表 Windows CI 或 Mac 实机已经通过；以交付时的 CI 结果和实测日志分别确认。

实机必须保持 Mac 正常 10 分钟自动关屏、不运行 caffeinate：

- Windows 后台空闲 15–20 分钟后返回，分别测试 HEVC30/60。
- 记录人工看到第一次有效画面和最终稳定画面的时间，核对 episode、观察结果及自动重连次数。
- 包含单帧静态 0 FPS、短暂恢复后再次操作、失焦/最小化再返回，以及真实网络断开。
- 手动 Disconnect 后不得自动再连接；拒绝认证后不得继续自动认证。
- 对比 e903d2e 的 42.159 秒，先区分恢复状态机等待、准备分段和呈现等待，再判断效果。

不从日志自动判定非黑或登录界面内容；无需截图或记录密码。恢复及准备诊断默认记录，无需开启逐帧日志。
