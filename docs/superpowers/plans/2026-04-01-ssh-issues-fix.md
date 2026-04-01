# SSH 回归问题修复实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复 3 个 SSH 回归问题：FsProvider dispose 缺失、连接断开无自动检测、上下文 ID 不匹配。

**Architecture:** 按依赖顺序实施：先给 FsProvider 添加异步 dispose 能力（04-03），再实现连接健康监控（04-01，依赖 dispose），最后统一上下文 ID 格式（04-02，独立）。

**Tech Stack:** Rust, Tauri v2, tokio, russh 0.46, russh-sftp, axum 0.8

**Spec:** `docs/superpowers/specs/2026-04-01-ssh-issues-fix-design.md`

---

## File Structure

| File | Action | Responsibility |
|------|--------|---------------|
| `src-tauri/src/infrastructure/fs_provider.rs` | Modify | 添加 `dispose()` 默认方法到 trait |
| `src-tauri/src/infrastructure/ssh_fs_provider.rs` | Modify | 添加 `dispose_async()` 和 `sftp_arc()` 方法 |
| `src-tauri/src/infrastructure/ssh_connection_manager.rs` | Modify | disconnect 中调用 dispose；connection 改为 Arc 包装；启动健康监控任务 |
| `src-tauri/src/commands/ssh.rs` | Modify | 动态 contextId 替换常量 |
| `src-tauri/src/http/routes/ssh.rs` | Modify | 动态 contextId 替换常量 |

---

## Task 1: FsProvider trait 添加 dispose 方法

**Files:**
- Modify: `src-tauri/src/infrastructure/fs_provider.rs:47`

- [ ] **Step 1: 在 FsProvider trait 中添加 `dispose` 默认方法**

在 `read_dir` 方法之后（第 64 行后）添加：

```rust
    /// 清理资源（如关闭 SFTP 会话）。默认为空操作。
    ///
    /// 对于需要异步清理的实现者（如 SshFsProvider），使用
    /// 自有的 `dispose_async()` 方法代替。
    fn dispose(&self) {}
```

- [ ] **Step 2: 验证编译通过**

Run: `cd /Users/liepin/Documents/github/claude-devtools-tauri && cargo check 2>&1 | tail -5`
Expected: `Finished` 无错误

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/infrastructure/fs_provider.rs
git commit -m "feat(fs_provider): add dispose() default method to trait (ISSUE-04-03)"
```

---

## Task 2: SshFsProvider 添加 dispose_async 和 sftp_arc 方法

**Files:**
- Modify: `src-tauri/src/infrastructure/ssh_fs_provider.rs:103-130`

- [ ] **Step 1: 在 SshFsProvider impl 块中添加两个方法**

在 `exists_async` 方法之后（第 129 行后）添加：

```rust
    /// 异步关闭 SFTP 会话。在 async 上下文中调用。
    ///
    /// 不使用 `block_on()`，因为调用方（disconnect、健康监控）
    /// 都是 async 函数，`block_on()` 会导致 panic。
    /// 错误仅记录 warn 日志，不传播（会话可能已关闭）。
    pub async fn dispose_async(&self) {
        if let Err(e) = self.sftp.close().await {
            log::warn!("SFTP close error (may be already closed): {}", e);
        }
    }

    /// Get a clone of the inner SFTP session Arc.
    /// Used by health monitor for SFTP probes.
    pub fn sftp_arc(&self) -> Arc<SftpSession> {
        self.sftp.clone()
    }
```

- [ ] **Step 2: 验证编译通过**

Run: `cd /Users/liepin/Documents/github/claude-devtools-tauri && cargo check 2>&1 | tail -5`
Expected: `Finished` 无错误

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/infrastructure/ssh_fs_provider.rs
git commit -m "feat(ssh_fs): add dispose_async() and sftp_arc() methods (ISSUE-04-03)"
```

---

## Task 3: disconnect 中调用 dispose_async

**Files:**
- Modify: `src-tauri/src/infrastructure/ssh_connection_manager.rs:282-304`

- [ ] **Step 1: 重构 `disconnect()` 方法，在断开前调用 dispose_async**

将现有的 `disconnect()` 方法（第 282-304 行）替换为：

```rust
    pub async fn disconnect(&self) -> Result<SshConnectionStatus, String> {
        // 先释放 SFTP 资源（在获取写锁之前）
        {
            let conn = self.connection.read().await;
            if let Some(ref connection) = *conn {
                connection.fs_provider.dispose_async().await;
            }
        }

        // 获取写锁，停止监控并断开连接
        let mut conn = self.connection.write().await;
        if conn.is_none() {
            return Ok(SshConnectionStatus::disconnected());
        }

        if let Some(ref mut connection) = *conn {
            // Signal monitor to stop
            let _ = connection.monitor_stop.send(true);

            // Graceful disconnect
            let _ = connection
                .session
                .disconnect(russh::Disconnect::ByApplication, "", "")
                .await;
        }

        *conn = None;
        let status = SshConnectionStatus::disconnected();
        let _ = self.event_sender.send(status.clone());

        Ok(status)
    }
```

关键变更：分离 dispose 和写锁操作。先读锁调用 `dispose_async()`，释放读锁，再获取写锁执行断开。

- [ ] **Step 2: 验证编译和现有测试通过**

Run: `cd /Users/liepin/Documents/github/claude-devtools-tauri/src-tauri && cargo test --lib infrastructure::ssh_connection_manager 2>&1 | tail -20`
Expected: 所有测试 PASS

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/infrastructure/ssh_connection_manager.rs
git commit -m "fix(ssh): call dispose_async before disconnect to release SFTP resources (ISSUE-04-03)"
```

---

## Task 4: 添加 get_connected_host 辅助方法

**Files:**
- Modify: `src-tauri/src/infrastructure/ssh_connection_manager.rs`

ISSUE-04-02 需要从已存储的连接获取 host。`get_active_state()` 在 disconnected 时 host 为 None，需要新方法。

- [ ] **Step 1: 在 `get_provider()` 方法之后添加 `get_connected_host()` 方法**

在第 444 行（`get_provider` 方法结束）之后添加：

```rust
    /// Get the host from the currently connected session's config.
    ///
    /// Returns `None` if not connected. Unlike `get_active_state().host`,
    /// this reads from the stored config (not the status snapshot),
    /// so it's available even during teardown.
    pub async fn get_connected_host(&self) -> Option<String> {
        let conn = self.connection.read().await;
        conn.as_ref().map(|c| c.config.host.clone())
    }
```

- [ ] **Step 2: 验证编译通过**

Run: `cd /Users/liepin/Documents/github/claude-devtools-tauri && cargo check 2>&1 | tail -5`
Expected: `Finished` 无错误

- [ ] **Step 3: Commit**

```bash
git add src-tauri/src/infrastructure/ssh_connection_manager.rs
git commit -m "feat(ssh): add get_connected_host() helper method"
```

---

## Task 5: 实现连接健康监控（ISSUE-04-01）

**Files:**
- Modify: `src-tauri/src/infrastructure/ssh_connection_manager.rs`

这是 ISSUE-04-01 的核心改动。需要将 `connection` 字段包装在 `Arc` 中，以便监控任务能清理它。

### 5.1: 添加常量

- [ ] **Step 1: 添加健康检查间隔常量**

在第 37 行 `CONNECT_TIMEOUT` 之后添加：

```rust
/// Health check interval (30 seconds).
const HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(30);
/// Health check SFTP probe timeout (10 seconds).
const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(10);
```

### 5.2: 将 connection 字段改为 Arc 包装

- [ ] **Step 2: 修改结构体字段**

在 `SshConnectionManager` 结构体定义中（第 89-96 行），将：
```rust
    connection: RwLock<Option<SshConnection>>,
```
改为：
```rust
    connection: Arc<RwLock<Option<SshConnection>>>,
```

- [ ] **Step 3: 更新 new() 构造器**

在 `new()` 方法中（第 117 行），将：
```rust
    connection: RwLock::new(None),
```
改为：
```rust
    connection: Arc::new(RwLock::new(None)),
```

- [ ] **Step 4: 更新测试构造器**

在测试中（第 936-938 行），将：
```rust
    connection: RwLock::const_new(None),
```
改为：
```rust
    connection: Arc::new(RwLock::const_new(None)),
```

同样修改第 957 行的测试构造器。

### 5.3: 添加 start_health_monitor 关联函数

- [ ] **Step 5: 在 `disconnect()` 方法之前添加 `start_health_monitor`**

在 `disconnect()` 方法之前（约第 275 行），添加：

```rust
    /// Start a background health monitor task for the current connection.
    ///
    /// The monitor polls every 30 seconds, checking:
    /// 1. `session.is_closed()` — russh internal task terminated
    /// 2. SFTP `metadata("/")` probe with 10s timeout
    ///
    /// On detection of disconnection:
    /// 1. Calls `dispose_async()` on the FsProvider
    /// 2. Sets `connection` to None
    /// 3. Broadcasts `Disconnected` status
    fn start_health_monitor(
        manager: Arc<Self>,
        session: client::Handle<SshClientHandler>,
        sftp: Arc<SftpSession>,
        event_sender: broadcast::Sender<SshConnectionStatus>,
        mut stop_rx: watch::Receiver<bool>,
    ) {
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = stop_rx.changed() => {
                        log::info!("SSH health monitor: stop signal received");
                        return;
                    }
                    _ = tokio::time::sleep(HEALTH_CHECK_INTERVAL) => {}
                }

                if *stop_rx.borrow() {
                    return;
                }

                // Health check 1: russh session internal state
                if session.is_closed() {
                    log::warn!("SSH health monitor: session is closed, connection lost");
                    break;
                }

                // Health check 2: SFTP probe
                let probe_result = tokio::time::timeout(
                    HEALTH_CHECK_TIMEOUT,
                    sftp.metadata("/"),
                ).await;

                match probe_result {
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => {
                        log::warn!("SSH health monitor: SFTP probe failed: {}, connection lost", e);
                        break;
                    }
                    Err(_) => {
                        log::warn!("SSH health monitor: SFTP probe timed out, connection lost");
                        break;
                    }
                }
            }

            // Health check failed — clean up
            log::info!("SSH health monitor: cleaning up disconnected session");

            // Dispose SFTP resources (read lock first)
            {
                let conn = manager.connection.read().await;
                if let Some(ref connection) = *conn {
                    connection.fs_provider.dispose_async().await;
                }
            }

            // Set connection to None (write lock)
            {
                let mut conn = manager.connection.write().await;
                *conn = None;
            }

            // Broadcast disconnected status
            let _ = event_sender.send(SshConnectionStatus::disconnected());
        });
    }
```

### 5.4: 在 connect() 中启动监控

- [ ] **Step 6: 修改 connect() 中的步骤 13-14**

将 `connect()` 中的步骤 13-14（第 253-267 行）替换为：

```rust
        // 13. Create monitor stop channel
        let (monitor_stop, monitor_stop_rx) = watch::channel(false);

        // 13.5. Clone resources for health monitor
        let monitor_session = session_mut.clone();
        let monitor_sftp = fs_provider.sftp_arc();
        let monitor_event_sender = self.event_sender.clone();

        // 14. Store the connection
        {
            let mut conn = self.connection.write().await;
            *conn = Some(SshConnection {
                config: merged_config,
                status: connected_status.clone(),
                remote_projects_path: Some(remote_projects_path),
                session: session_mut,
                fs_provider,
                monitor_stop,
            });
        }
```

**注意**: `start_health_monitor` 需要 `Arc<Self>`，但 `connect()` 接收 `&self`。需要在调用层（`commands/ssh.rs` 和 `http/routes/ssh.rs`）传入 `Arc<SshConnectionManager>` 并调用。但 `SshConnectionManager` 目前由 `Arc<RwLock<SshConnectionManager>>` 包装。

**解决方案**: 将 `connect()` 和 `disconnect()` 改为接收 `Arc<Self>` 而非 `&self`，或者将 `start_health_monitor` 的启动移到调用层。

**最终方案**: 在 `SshConnectionManager` 上添加 `set_manager_arc` 方法，在初始化时存储自身的 `Arc`（弱引用），或者在 `connect()` 中通过传参方式传入。最简单的做法：

在 `SshConnection` 中不启动监控，改为在调用层（`commands/ssh.rs` 的 `ssh_connect`）中启动。这样调用层有 `Arc<RwLock<SshConnectionManager>>` 可以克隆。

但这增加了调用层的复杂度。更简单的方案：在 `SshConnectionManager` 中存储一个 `Weak<Self>` 字段。

**最终决定**: 在 `connect()` 中暂时不启动监控（留到下一步），先确保 `Arc<RwLock<...>>` 改动正确编译。然后在 Task 5.5 中处理启动逻辑。

- [ ] **Step 7: 添加 `start_monitor_after_connect` 方法**

在 `connect()` 方法中不启动监控，而是提供一个独立方法供调用层使用：

```rust
    /// Start the health monitor for the current connection.
    ///
    /// Must be called after `connect()` succeeds. Requires `Arc<Self>`
    /// to allow the monitor task to clean up on disconnect detection.
    pub fn start_monitor(self: &Arc<Self>) {
        let conn = self.connection.try_read();
        if let Ok(Some(connection)) = conn.as_ref().map(|c| c.as_ref()) {
            let stop_rx = connection.monitor_stop.subscribe();
            let session = connection.session.clone();
            let sftp = connection.fs_provider.sftp_arc();
            let event_sender = self.event_sender.clone();

            Self::start_health_monitor(
                Arc::clone(self),
                session,
                sftp,
                event_sender,
                stop_rx,
            );
        }
    }
```

**问题**: `watch::Sender` 没有 `subscribe()` 方法，`subscribe` 是 `watch::Receiver` 的。且 `monitor_stop` 是 `Sender`，`Receiver` 在创建 channel 时产生。

**最终方案（简洁版）**: 改用 `oneshot` 通道代替 `watch` 通道，或者直接在 `connect()` 方法内部使用 `self` 来 clone Arc。

**实际可行的方案**: `SshConnectionManager` 被 `Arc<RwLock<SshConnectionManager>>` 包装。在 `commands/ssh.rs` 中：

```rust
let manager_arc = ssh_manager.read().await; // Arc<RwLock<..>>
let guard = manager_arc.read().await; // 获取 &SshConnectionManager
```

但这样无法获得 `Arc<SshConnectionManager>`。

**真正可行的方案**: 将 `SshConnectionManager` 从 `Arc<RwLock<...>>` 改为 `Arc<...>`，将内部 `RwLock` 移到 `SshConnectionManager` 自己的字段上（已经是了）。

也就是说，将 `ssh_manager: State<'_, Arc<RwLock<SshConnectionManager>>>` 改为 `ssh_manager: State<'_, Arc<SshConnectionManager>>`。

这是更大的重构。**为了最小化改动，我们采用"在调用层启动监控"的方式**：

在 `commands/ssh.rs` 和 `http/routes/ssh.rs` 中，`connect` 成功后启动监控任务，传入 `Arc<RwLock<SshConnectionManager>>` 和必要的克隆资源。

- [ ] **Step 8: 添加 `start_health_monitor_external` 关联函数**

替代 Step 7 的方案。在 `ssh_connection_manager.rs` 中添加一个可从外部调用的方法：

```rust
    /// Start health monitor from the calling layer.
    ///
    /// Called after `connect()` succeeds. The monitor will probe the
    /// connection every 30s and auto-clean on disconnect detection.
    pub async fn start_health_monitor_from_arc(
        manager: Arc<RwLock<Self>>,
        event_sender: broadcast::Sender<SshConnectionStatus>,
    ) {
        let (session, sftp, stop_rx) = {
            let mgr = manager.read().await;
            let conn = mgr.connection.read().await;
            match conn.as_ref() {
                Some(c) => (
                    c.session.clone(),
                    c.fs_provider.sftp_arc(),
                    // Create a receiver from the stored sender
                    watch::channel(false).1, // 问题：不能从 sender 创建 receiver
                ),
                None => return,
            }
        };
        // ...
    }
```

**问题**: `watch::Sender` 不能 clone 出 `Receiver`。需要在创建 channel 时就保存 Receiver。

**真正的最终方案**: 将 `SshConnection` 中的 `monitor_stop: watch::Sender<bool>` 改为同时保存 Receiver：

```rust
struct SshConnection {
    // ... existing fields ...
    monitor_stop: watch::Sender<bool>,
    monitor_stop_rx: watch::Receiver<bool>,  // 添加
}
```

但这会导致 `SshConnection` 不再是 `Send`（`watch::Receiver` 不是 `Sync`）。

**最终方案（确信可行）**: 使用 `tokio::sync::watch` 的方式，在 `connect()` 中创建 `(sender, receiver)` 对，sender 存入 `SshConnection`，receiver 传给监控任务。然后在 `start_health_monitor` 中传入 `Arc<RwLock<SshConnectionManager>>`（而非 `Arc<SshConnectionManager>`），这样调用层无需改动包装类型。

- [ ] **Step 9: 实现最终版健康监控**

在 `ssh_connection_manager.rs` 中，在 `disconnect()` 方法之前添加：

```rust
    /// Start a background health monitor task.
    ///
    /// Called by the command/route layer after `connect()` succeeds.
    /// The `manager` parameter is the outer `Arc<RwLock<Self>>` wrapper
    /// from the Tauri state, allowing the monitor to clean up on failure.
    fn start_health_monitor(
        manager: Arc<RwLock<Self>>,
        session: client::Handle<SshClientHandler>,
        sftp: Arc<SftpSession>,
        event_sender: broadcast::Sender<SshConnectionStatus>,
        mut stop_rx: watch::Receiver<bool>,
    ) {
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = stop_rx.changed() => {
                        log::info!("SSH health monitor: stop signal received");
                        return;
                    }
                    _ = tokio::time::sleep(HEALTH_CHECK_INTERVAL) => {}
                }

                if *stop_rx.borrow() {
                    return;
                }

                if session.is_closed() {
                    log::warn!("SSH health monitor: session closed, connection lost");
                    break;
                }

                let probe = tokio::time::timeout(
                    HEALTH_CHECK_TIMEOUT,
                    sftp.metadata("/"),
                ).await;

                match probe {
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => {
                        log::warn!("SSH health monitor: SFTP probe failed: {}, connection lost", e);
                        break;
                    }
                    Err(_) => {
                        log::warn!("SSH health monitor: SFTP probe timed out, connection lost");
                        break;
                    }
                }
            }

            // Health check failed — clean up
            log::info!("SSH health monitor: cleaning up disconnected session");
            let mgr = manager.read().await;

            // Dispose SFTP resources
            {
                let conn = mgr.connection.read().await;
                if let Some(ref connection) = *conn {
                    connection.fs_provider.dispose_async().await;
                }
            }

            // Set connection to None
            {
                let mut conn = mgr.connection.write().await;
                *conn = None;
            }

            let _ = event_sender.send(SshConnectionStatus::disconnected());
        });
    }

    /// Public entry point for starting the health monitor after connect.
    pub fn spawn_health_monitor(
        manager: Arc<RwLock<Self>>,
        session: client::Handle<SshClientHandler>,
        sftp: Arc<SftpSession>,
        event_sender: broadcast::Sender<SshConnectionStatus>,
        stop_rx: watch::Receiver<bool>,
    ) {
        Self::start_health_monitor(manager, session, sftp, event_sender, stop_rx);
    }
```

- [ ] **Step 10: 在调用层启动监控（commands/ssh.rs）**

在 `commands/ssh.rs` 的 `ssh_connect` 中，`Ok(status)` 返回之前，添加：

```rust
    // 5. Start health monitor
    {
        let mgr = ssh_manager.read().await;
        let conn = mgr.connection.read().await;
        if let Some(ref connection) = *conn {
            let monitor_stop_rx = watch::channel(false).1;
            // 问题：我们没法从已有的 sender 创建 receiver
        }
    }
```

**问题复现**: `connect()` 方法内部创建了 `watch::channel(false)` 并把 sender 存入 `SshConnection`，但 receiver 已被丢弃。

**解决方案**: 修改 `connect()` 方法，返回 `monitor_stop_rx` 给调用者。

将 `connect()` 的返回类型改为包含 monitor receiver：

```rust
    pub async fn connect(
        &self,
        config: SshConnectionConfig,
    ) -> Result<(SshConnectionStatus, Option<watch::Receiver<bool>>), String> {
```

在步骤 13 中：
```rust
        // 13. Create monitor stop channel
        let (monitor_stop, monitor_stop_rx) = watch::channel(false);
```

在返回时：
```rust
        Ok((connected_status, Some(monitor_stop_rx)))
```

所有错误返回路径改为 `Ok((error_status, None))`。

在 `commands/ssh.rs` 调用处：
```rust
    let (status, monitor_rx) = ssh_manager.write().await.connect(config).await?;
    // ... context switch logic ...

    // Start health monitor
    if let Some(stop_rx) = monitor_rx {
        SshConnectionManager::spawn_health_monitor(
            Arc::clone(&ssh_manager),
            /* session clone */ ...,
            /* sftp clone */ ...,
            /* event_sender */ ...,
            stop_rx,
        );
    }
```

**问题**: 调用层需要 session 和 sftp 的 clone。这些在 `connect()` 内部创建后就存入了 `SshConnection`，调用层无法访问。

**真正的最终方案（简化）**: 在 `connect()` 方法内直接启动监控。使用 `Arc::clone(&self.connection)` 将共享的 connection 传给监控任务。`self.connection` 已经是 `Arc<RwLock<...>>`（Step 2 改的）。

监控任务需要 `event_sender`（从 `self` clone）和 `connection`（从 `self` clone）。唯一的问题是监控任务无法停止——但它可以通过 `watch::Receiver` 停止，只需将 receiver 也存入 connection 或作为参数传给 spawn。

**最终实现（在 connect() 内启动）**: 修改步骤 13-14 为：

```rust
        // 13. Create monitor stop channel
        let (monitor_stop, monitor_stop_rx) = watch::channel(false);

        // 13.5. Clone resources for health monitor
        let monitor_session = session_mut.clone();
        let monitor_sftp = fs_provider.sftp_arc();
        let monitor_event_sender = self.event_sender.clone();
        let monitor_connection = Arc::clone(&self.connection);

        // 14. Store the connection
        {
            let mut conn = self.connection.write().await;
            *conn = Some(SshConnection {
                config: merged_config,
                status: connected_status.clone(),
                remote_projects_path: Some(remote_projects_path),
                session: session_mut,
                fs_provider,
                monitor_stop,
            });
        }

        // 14.5. Start health monitor (inside connect, using cloned resources)
        {
            let manager_connection = monitor_connection;
            let session = monitor_session;
            let sftp = monitor_sftp;
            let sender = monitor_event_sender;
            let mut stop_rx = monitor_stop_rx;

            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = stop_rx.changed() => {
                            log::info!("SSH health monitor: stop signal received");
                            return;
                        }
                        _ = tokio::time::sleep(HEALTH_CHECK_INTERVAL) => {}
                    }

                    if *stop_rx.borrow() {
                        return;
                    }

                    if session.is_closed() {
                        log::warn!("SSH health monitor: session closed, connection lost");
                        break;
                    }

                    let probe = tokio::time::timeout(
                        HEALTH_CHECK_TIMEOUT,
                        sftp.metadata("/"),
                    ).await;

                    match probe {
                        Ok(Ok(_)) => {}
                        Ok(Err(e)) => {
                            log::warn!("SSH health monitor: SFTP probe failed: {}, connection lost", e);
                            break;
                        }
                        Err(_) => {
                            log::warn!("SSH health monitor: SFTP probe timed out, connection lost");
                            break;
                        }
                    }
                }

                // Clean up on disconnect
                log::info!("SSH health monitor: cleaning up");
                {
                    let conn = manager_connection.read().await;
                    if let Some(ref c) = *conn {
                        c.fs_provider.dispose_async().await;
                    }
                }
                {
                    let mut conn = manager_connection.write().await;
                    *conn = None;
                }
                let _ = sender.send(SshConnectionStatus::disconnected());
            });
        }
```

**这个方案可行**，因为：
- `self.connection` 是 `Arc<RwLock<...>>`，可以 `Arc::clone` 传给 spawn
- `session.clone()` 和 `fs_provider.sftp_arc()` 在 spawn 之前获取
- `event_sender.clone()` 是 broadcast sender，可以 clone
- `monitor_stop_rx` 是 watch receiver，在创建 channel 时就获得了
- 不需要 `Arc<SshConnectionManager>` 或改变调用层

- [ ] **Step 11: 验证编译通过**

Run: `cd /Users/liepin/Documents/github/claude-devtools-tauri && cargo check 2>&1 | tail -5`
Expected: `Finished` 无错误

- [ ] **Step 12: 运行 SSH 连接管理器测试**

Run: `cd /Users/liepin/Documents/github/claude-devtools-tauri/src-tauri && cargo test --lib infrastructure::ssh_connection_manager 2>&1 | tail -20`
Expected: 所有测试 PASS

- [ ] **Step 13: Commit**

```bash
git add src-tauri/src/infrastructure/ssh_connection_manager.rs src-tauri/src/infrastructure/ssh_fs_provider.rs
git commit -m "feat(ssh): add connection health monitor with auto-disconnect detection (ISSUE-04-01)

- Wrap connection field in Arc for monitor task access
- Add health monitor with 30s polling interval
- Use session.is_closed() + SFTP metadata probe for health checks
- Auto-cleanup (dispose + set None) on disconnect detection"
```

---

## Task 6: SSH 上下文 ID 统一 — commands/ssh.rs

**Files:**
- Modify: `src-tauri/src/commands/ssh.rs`

- [ ] **Step 1: 移除常量，添加辅助函数**

移除第 24 行的 `const SSH_CONTEXT_ID: &str = "ssh";`，替换为：

```rust
/// Construct dynamic SSH context ID from host.
fn ssh_context_id(host: &str) -> String {
    if host.is_empty() {
        "ssh".to_string()
    } else {
        format!("ssh-{}", host)
    }
}

/// Check if a context ID belongs to an SSH context.
fn is_ssh_context_id(id: &str) -> bool {
    id == "ssh" || id.starts_with("ssh-")
}
```

- [ ] **Step 2: 更新 `ssh_connect` 中的 context ID 使用**

在 `ssh_connect` 函数中：

1. 第 70 行 `id: SSH_CONTEXT_ID.to_string()` → `id: ssh_context_id(&host)`

2. 第 82 行 `if mgr.get_active_id() == SSH_CONTEXT_ID` → `if is_ssh_context_id(mgr.get_active_id())`

3. 第 85 行 `if let Some(ssh_ctx) = mgr.get(SSH_CONTEXT_ID)` → 使用动态 ID：
```rust
            let active_id = mgr.get_active_id().to_string();
            if let Some(ssh_ctx) = mgr.get(&active_id) {
```

4. 第 105 行 `let _ = mgr.destroy_context(SSH_CONTEXT_ID).await;` → `let _ = mgr.destroy_context(&active_id).await;`

5. 第 116 行 `let result = mgr.switch(SSH_CONTEXT_ID)?;` → `let result = mgr.switch(&ssh_context_id(&host))?;`

- [ ] **Step 3: 更新 `ssh_disconnect` 中的 context ID 使用**

1. 第 180 行 `if mgr.get_active_id() != SSH_CONTEXT_ID` → `if !is_ssh_context_id(mgr.get_active_id())`

2. 在 `switch("local")` 附近，保存 `result.previous_id`，然后用它来 destroy：

找到 `mgr.switch("local")?;`（约第 191 行），确认已有 `let result = mgr.switch("local")?;`。然后将第 224 行 `mgr.destroy_context(SSH_CONTEXT_ID).await?;` 改为：

```rust
            mgr.destroy_context(&result.previous_id).await?;
```

**关键**: `destroy_context` 在 `mgr.switch("local")` 之后调用。此时 `get_active_id()` 已变为 `"local"`，所以**不能**用 `get_active_id()`。必须用 `switch()` 返回的 `result.previous_id`。

- [ ] **Step 4: 验证编译通过**

Run: `cd /Users/liepin/Documents/github/claude-devtools-tauri && cargo check 2>&1 | tail -5`
Expected: `Finished` 无错误

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/commands/ssh.rs
git commit -m "fix(ssh): use dynamic ssh-{host} context ID in IPC commands (ISSUE-04-02)"
```

---

## Task 7: SSH 上下文 ID 统一 — http/routes/ssh.rs

**Files:**
- Modify: `src-tauri/src/http/routes/ssh.rs`

- [ ] **Step 1: 移除常量，添加辅助函数**

移除第 32 行的 `const SSH_CONTEXT_ID: &str = "ssh";`，替换为：

```rust
/// Construct dynamic SSH context ID from host.
fn ssh_context_id(host: &str) -> String {
    if host.is_empty() {
        "ssh".to_string()
    } else {
        format!("ssh-{}", host)
    }
}

/// Check if a context ID belongs to an SSH context.
fn is_ssh_context_id(id: &str) -> bool {
    id == "ssh" || id.starts_with("ssh-")
}
```

- [ ] **Step 2: 更新 `ssh_connect` 中的 context ID 使用**

1. 第 85 行 `id: SSH_CONTEXT_ID.to_string()` → `id: ssh_context_id(&host)`

2. 第 97 行 `if mgr.get_active_id() == SSH_CONTEXT_ID` → `if is_ssh_context_id(mgr.get_active_id())`

3. 第 99 行 `if let Some(ssh_ctx) = mgr.get(SSH_CONTEXT_ID)` → 使用动态 ID：
```rust
            let active_id = mgr.get_active_id().to_string();
            if let Some(ssh_ctx) = mgr.get(&active_id) {
```

4. 第 103 行 `let _ = mgr.destroy_context(SSH_CONTEXT_ID).await;` → `let _ = mgr.destroy_context(&active_id).await;`

5. 第 114 行 `let result = mgr.switch(SSH_CONTEXT_ID).map_err(error_json)?;` → `let result = mgr.switch(&ssh_context_id(&host)).map_err(error_json)?;`

- [ ] **Step 3: 更新 `ssh_disconnect` 中的 context ID 使用**

1. 第 158 行 `mgr.get_active_id() == SSH_CONTEXT_ID` → `is_ssh_context_id(mgr.get_active_id())`

2. `destroy_context` 必须使用 `result.previous_id`（与 Task 6 相同的模式）：

找到 `mgr.switch("local").map_err(error_json)?;`，确认已保存为 `let result = ...`。然后将第 198-199 行 `mgr.destroy_context(SSH_CONTEXT_ID)` 改为：

```rust
            mgr.destroy_context(&result.previous_id)
                .await
                .map_err(error_json)?;
```

**关键**: 不能用 `get_active_id()`，因为 switch 后已变为 `"local"`。

- [ ] **Step 4: 验证编译通过**

Run: `cd /Users/liepin/Documents/github/claude-devtools-tauri && cargo check 2>&1 | tail -5`
Expected: `Finished` 无错误

- [ ] **Step 5: 运行全量测试**

Run: `cd /Users/liepin/Documents/github/claude-devtools-tauri/src-tauri && cargo test --lib 2>&1 | tail -30`
Expected: 所有测试 PASS

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/http/routes/ssh.rs
git commit -m "fix(ssh): use dynamic ssh-{host} context ID in HTTP routes (ISSUE-04-02)"
```

---

## Task 8: 集成验证

- [ ] **Step 1: 运行完整 cargo check**

Run: `cd /Users/liepin/Documents/github/claude-devtools-tauri && cargo check 2>&1 | tail -5`
Expected: `Finished` 无错误

- [ ] **Step 2: 运行全量测试**

Run: `cd /Users/liepin/Documents/github/claude-devtools-tauri/src-tauri && cargo test --lib 2>&1 | tail -30`
Expected: 所有测试 PASS

- [ ] **Step 3: 运行前端构建验证**

Run: `cd /Users/liepin/Documents/github/claude-devtools-tauri && pnpm build 2>&1 | tail -10`
Expected: 构建成功
