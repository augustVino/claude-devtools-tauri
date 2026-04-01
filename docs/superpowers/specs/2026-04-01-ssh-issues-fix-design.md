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

**2. 健康检查方式:**
- 优先使用 `session.is_alive()` 检查 russh session 存活
- 若 `is_alive()` 不可靠，回退到轻量 SFTP 操作（`sftp.stat("/")`）
- 检查超时 10 秒（复用 `CONNECT_TIMEOUT` 参考值）

**3. 检测到断开时的处理:**
- 通过 `event_sender` 广播 `SshConnectionStatus::disconnected()`
- 获取写锁，将 `connection` 字段设为 `None`
- 前端通过已有的 `"ssh:status"` 事件链路自动收到通知

**4. 涉及文件:**
- `src-tauri/src/infrastructure/ssh_connection_manager.rs` — 核心改动

---

## ISSUE-04-02: SSH 上下文 ID 统一

### 问题

后端用常量 `"ssh"` 注册 context，前端 `connectionSlice` 用 `` `ssh-${host}` `` 设置 `activeContextId`。导致：
- `store/index.ts` 的 `context:changed` 监听器总是发现不匹配，触发多余 `switchContext`
- `WorkspaceIndicator`、`ConnectionStatusBadge` 等组件依赖 `ssh-` 前缀格式解析主机名

### 设计

修改后端使用 `ssh-${host}` 动态格式，对齐 Electron 和前端组件。

**1. 后端改动:**
- `commands/ssh.rs`: 移除 `const SSH_CONTEXT_ID: &str = "ssh"` 常量
  - `connect_ssh()` 中从 `status.host` 动态构造 context_id: `` format!("ssh-{}", host) ``
  - `disconnect_ssh()` 中从 `ssh_manager.get_active_state()` 获取 host 构造 ID
- `http/routes/ssh.rs`: 同上

**2. 前端无需改动:**
- `connectionSlice.ts` 已经使用 `` `ssh-${config.host}` ``
- 组件已经期望 `ssh-` 前缀格式

**3. 边界情况:**
- 如果 `host` 为 `None`，回退到 `"ssh"` 作为 context_id
- `destroy_context` 时用动态 ID 而非常量

**4. 涉及文件:**
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
- `LocalFsProvider`: `dispose()` 为 no-op（无需清理）
- `SshFsProvider`: `dispose()` 主动关闭 SFTP 会话

**3. 调用点:**
- `ssh_connection_manager.rs` 的 `disconnect()`: 在断开前调用 `connection.fs_provider.dispose()`
- 可选: `context_manager.rs` 的 `destroy_context()` 中调用

**4. 不补齐 `createReadStream`:**
- Tauri 已用 `read_file_head` 替代流式读取
- FileWatcher 增量读取使用不同的实现策略，无需 `createReadStream`

**5. 涉及文件:**
- `src-tauri/src/infrastructure/fs_provider.rs` — trait 添加方法
- `src-tauri/src/infrastructure/ssh_fs_provider.rs` — 实现 dispose
- `src-tauri/src/infrastructure/ssh_connection_manager.rs` — disconnect 中调用 dispose
