# SSH 回归问题修复设计

> 日期: 2026-04-01
> 来源: `docs/checks/check-04-ssh.md`
> 范围: ISSUE-04-01 (HIGH), ISSUE-04-02 (HIGH), ISSUE-04-03 (MEDIUM)
> 跳过: ISSUE-04-04/05/06 (LOW, 功能等效)

---

## ISSUE-04-01: SSH 连接断开自动检测

### 问题

Tauri 端没有 SSH 连接健康监控。连接意外断开后，`SshConnection` 保持 `connected` 状态，直到下一次操作失败。`monitor_stop: watch::Sender<bool>` 字段已预留但从未使用（死代码）。

### 设计

在 `connect()` 成功后启动异步健康监控任务：

**1. 监控任务生命周期:**
- 在 `connect()` 步骤 13（创建 `watch::channel(false)`）之后、步骤 14（存入 connection）之前启动
- 从 `watch::channel` 获取 `Receiver<bool>` 监听停止信号
- 30 秒间隔轮询连接存活状态
- `disconnect()` 中已有的 `monitor_stop.send(true)` 会自动停止监控

**2. 监控任务资源获取方式:**
- 监控任务需要持有 `Arc<SftpSession>` 副本和 `client::Handle` 副本用于健康检查
- 这些引用在 `connect()` 中已有，在启动监控任务前 clone 并 move 进任务
- 监控任务还需持有 `event_sender` 的 clone 用于广播断开状态
- 通过 `Arc<SshConnectionManager>` 的 weak 引用或直接传参来触发 `connection` 字段的清理

**3. 健康检查方式:**
- **主方案**: 使用 `session.is_closed()` 检测 russh 后台任务是否已终止（注意：`is_closed()` 检测的是内部 channel sender 是否 dropped，不是 TCP 层面存活）
- **补充方案**: 每次轮询时执行轻量 SFTP 操作（`sftp.stat("/")`），通过 `tokio::time::timeout` 设置 10 秒超时
- 如果 `is_closed()` 返回 `true` 或 SFTP 操作超时/失败，判定连接已断开
- 注意：russh 0.46 的 `client::Handle` 没有 `is_alive()` 方法

**4. 检测到断开时的处理:**
- 通过 `event_sender` 广播 `SshConnectionStatus::disconnected()`
- 获取写锁，先对 `connection.fs_provider` 调用 `dispose()` 释放 SFTP 资源，然后将 `connection` 字段设为 `None`
- 前端通过已有的 `"ssh:status"` 事件链路自动收到通知

**5. 涉及文件:**
- `src-tauri/src/infrastructure/ssh_connection_manager.rs` — 核心改动

---

## ISSUE-04-02: SSH 上下文 ID 统一

### 问题

后端用常量 `"ssh"` 注册 context，前端 `connectionSlice` 用 `` `ssh-${host}` `` 设置 `activeContextId`。导致：
- `store/index.ts` 的 `context:changed` 监听器总是发现不匹配，触发多余 `switchContext`
- `WorkspaceIndicator`、`ConnectionStatusBadge` 等组件依赖 `ssh-` 前缀格式解析主机名

### 设计

修改后端使用 `ssh-${host}` 动态格式，对齐 Electron 和前端组件。

**1. 辅助函数:**
```rust
/// 从 host 构造动态 SSH context ID
fn ssh_context_id(host: &str) -> String {
    format!("ssh-{}", host)
}

/// 判断 context ID 是否为 SSH 类型（前缀匹配）
fn is_ssh_context_id(id: &str) -> bool {
    id.starts_with("ssh-")
}
```

**2. 后端改动 — `commands/ssh.rs`:**
- 移除 `const SSH_CONTEXT_ID: &str = "ssh"` 常量
- `connect_ssh()` 中：
  - 从 `status.host` 构造 context_id: `ssh_context_id(host)`
  - reconnect 检测：将 `mgr.get_active_id() == SSH_CONTEXT_ID` 改为 `is_ssh_context_id(&active_id)`，然后用 `active_id` 本身作为 `destroy_context` 的参数
- `disconnect_ssh()` 中：
  - 从存储的 `connection.config.host`（非 `get_active_state()`）获取 host 构造动态 ID
  - 原因：`get_active_state()` 在 disconnected 时 host 为 None，而 `connection.config.host` 在连接建立时就已保存

**3. 后端改动 — `http/routes/ssh.rs`:**
- 同 `commands/ssh.rs` 的改动模式
- 移除重复的 `SSH_CONTEXT_ID` 常量
- 所有相等比较改为前缀匹配

**4. 前端无需改动:**
- `connectionSlice.ts` 已经使用 `` `ssh-${config.host}` ``
- 组件已经期望 `ssh-` 前缀格式

**5. 边界情况:**
- 如果 `host` 为空字符串，回退到 `"ssh"` 作为 context_id
- reconnect 同一 host 时：先 destroy 旧 context（ID 相同），再 register 新 context，保持现有流程的先销毁后注册顺序
- 不同 host 重连：新旧 ID 不同，`register_context` 不会冲突

**6. 涉及文件:**
- `src-tauri/src/commands/ssh.rs`
- `src-tauri/src/http/routes/ssh.rs`

---

## ISSUE-04-03: FsProvider 添加 dispose 方法

### 问题

`FsProvider` trait 缺少 `dispose()` 方法。SFTP 会话依赖 `Arc` 引用计数自动释放，无主动断连，可能导致资源泄漏。

### 设计

**1. Trait 扩展:**
```rust
// fs_provider.rs
pub trait FsProvider: Send + Sync + std::fmt::Debug {
    // ... 现有方法 ...
    fn dispose(&self) {} // 默认空实现
}
```

**2. 实现者:**
- `LocalFsProvider`: `dispose()` 为 no-op（无需清理，使用默认实现）
- `SshFsProvider`: `dispose()` 使用 `self.handle.block_on(self.sftp.close())` 关闭 SFTP 会话
  - 已有 `handle: tokio::runtime::Handle` 字段，与 trait 其他方法一致
  - 错误处理：`close()` 失败仅记录日志，不传播错误
  - 注意：不能在 tokio 异步上下文中调用 `block_on()`，否则会 panic。`disconnect()` 目前是 `async fn`，需要确保在进入异步运行时之前或通过 `tokio::task::spawn_blocking` 调用

**3. 调用点:**
- `ssh_connection_manager.rs` 的 `disconnect()`: 在 disconnect 包发送前，对 `connection.fs_provider.dispose()` 调用（直接在 `SshConnection` 具体类型上调用，非 trait object）
- ISSUE-04-01 的健康监控任务检测到断开时也需调用 `fs_provider.dispose()`（见 04-01 设计第 4 点）

**4. 不补齐 `createReadStream`:**
- Tauri 已用 `read_file_head` 替代流式读取
- FileWatcher 增量读取使用不同的实现策略，无需 `createReadStream`

**5. 涉及文件:**
- `src-tauri/src/infrastructure/fs_provider.rs` — trait 添加方法
- `src-tauri/src/infrastructure/ssh_fs_provider.rs` — 实现 dispose
- `src-tauri/src/infrastructure/ssh_connection_manager.rs` — disconnect 中调用 dispose

---

## 跨 ISSUE 依赖

1. **04-03 必须先于 04-01 实现**: 健康监控任务检测到断开时需要调用 `dispose()`，因此 `dispose()` 方法必须先存在
2. **04-02 独立于 04-01 和 04-03**: context ID 统一与其他两个修复无直接依赖，但建议最后实现以减少并发修改的风险
3. **建议实施顺序**: 04-03 → 04-01 → 04-02
