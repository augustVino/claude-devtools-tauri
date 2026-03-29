# Architecture

## Backend Modules (`src-tauri/src/`)

```
lib.rs              # 入口，插件注册，70+ Tauri commands 通过 generate_handler! 注册
├── commands/       # IPC 命令处理 (sessions, projects, search, config, notifications, updater, ssh, context, http_server 等)
├── types/          # 类型定义
│   ├── jsonl.rs    # ChatHistoryEntry 枚举，ContentBlock (Text/Thinking/ToolUse/ToolResult/Image)
│   ├── domain.rs   # Session, Project, IpcResult<T>, PaginatedSessionsResult
│   ├── chunks.rs   # Chunk 枚举 (User/Ai/System/Compact)，AiChunk 含 ToolExecution/SemanticStep/Process
│   ├── messages.rs # ParsedMessage (Rust 侧解析结果)
│   ├── config.rs   # AppConfig, NotificationTrigger, StoredNotification, DetectedError
│   └── ssh.rs      # SshConnectionConfig, SshConnectionStatus, SshConfigHostEntry 等
├── parsing/        # JSONL 解析管道
│   ├── jsonl_parser.rs         # 逐行读取 JSONL → ChatHistoryEntry
│   ├── message_classifier.rs   # 消息分类 → MessageCategory
│   ├── session_parser.rs       # 编排解析，产出 ParsedSession
│   ├── agent_config_reader.rs  # .claude/agents/ 配置解析
│   ├── claude_md_reader.rs     # CLAUDE.md 文件读取
│   └── git_identity.rs         # Git 身份提取
├── analysis/       # 分析管道
│   ├── chunk_builder.rs            # 消息 → 可视化 Chunk（核心）
│   ├── tool_execution_builder.rs   # 配对 tool_use ↔ tool_result
│   ├── semantic_step_extractor.rs  # 从 AI 块提取语义步骤
│   ├── process_linker.rs           # 关联子 Agent 进程到父 AI 块
│   ├── conversation_group_builder.rs # 对话分组构建
│   └── waterfall_builder.rs        # 瀑布图数据构建
├── discovery/      # 项目扫描、会话搜索、子 Agent 解析、worktree 分组
├── error/          # 错误检测管道 (ErrorDetector → ErrorTriggerChecker → NotificationManager)
├── infrastructure/ # 基础设施
│   ├── config_manager.rs          # Config 读写，深度合并默认值
│   ├── data_cache.rs              # moka LRU 缓存 (50 条, 10 min TTL)
│   ├── file_watcher.rs            # notify-debouncer-mini (100ms debounce)
│   ├── notification_manager.rs    # 通知持久化，原生通知节流 (5s/唯一 hash)
│   ├── trigger_manager.rs         # Trigger CRUD，ReDoS 防护，3 个内置默认触发器
│   ├── context_manager.rs         # 多上下文注册/切换/销毁
│   ├── service_context.rs         # ServiceContext（每个上下文的服务栈）
│   ├── fs_provider.rs             # FsProvider trait + LocalFsProvider
│   ├── ssh_connection_manager.rs  # SSH 连接生命周期 (russh)
│   ├── ssh_config_parser.rs       # ~/.ssh/config 解析
│   └── ssh_fs_provider.rs         # SFTP FsProvider（Phase 1 占位）
├── http/            # Axum HTTP 服务模块
│   ├── server.rs    # 服务生命周期，端口扫描 (3456-3466)
│   ├── sse.rs       # SSEBroadcaster (broadcast channel, capacity 256)
│   ├── state.rs     # HttpState（Axum 共享状态）
│   ├── cors.rs      # CORS 配置
│   └── routes/      # 12 个路由模块
├── utils/           # content_sanitizer, context_accumulator, regex_validation, session_state_detection, timeline_gap_filling
└── events.rs        # file-change, todo-change, notification:new/updated, error:detected, context:changed
```

**共享状态**: `Arc<RwLock<AppState>>` (cache + config_manager)，`Arc<RwLock<ContextManager>>`，`Arc<RwLock<SshConnectionManager>>`，`SSEBroadcaster`，`HttpServerHandle`。

## Multi-Context Architecture

`ContextManager` 管理多个 `ServiceContext` 实例（本地 + SSH）。每个上下文封装独立的 scanner、searcher、cache 和 file watchers。切换上下文时停止旧 watchers 并启动新的。

`FsProvider` 同步 trait 抽象文件系统操作：`LocalFsProvider`（本地）和 `SshFsProvider`（占位中），要求 `Send + Sync`。

前端 `contextStorage.ts` 用 IndexedDB 保存快照（5 min TTL）实现秒切。

## Frontend Structure (`src/`)

```
api/                  # 双传输层：TauriIPC vs HTTP+SSE，通过 ElectronAPI 接口统一契约
store/                # Zustand 5，14 个切片 (project, repository, session, sessionDetail, subagent,
                     # conversation, tab, tabUI, pane, ui, notification, config, connection, context, update)
services/contextStorage.ts  # IndexedDB 上下文快照持久化
components/
├── chat/             # 会话展示 + linkedTool viewers
├── common/           # ConnectionStatusBadge, ContextSwitchOverlay, UpdateBanner/Dialog 等
├── layout/           # TabbedLayout, PaneContainer, PaneView, TabBar (拖拽)
├── sidebar/          # 日期分组会话列表
├── settings/         # 设置 UI（含 ConnectionSection SSH 配置）
├── search/           # CommandPalette, SearchBar
├── dashboard/        # DashboardView
└── notifications/    # NotificationsView
```

**三层类型系统**: `src/main/types/` (镜像 Rust) → `src/shared/types/` (跨进程契约) → `src/types/` (渲染器专用)

## Data Flows

**命令流** (请求-响应): React → Zustand action → `api.xxx()` → `TauriAPIClient.invoke('xxx')` → Rust command → 返回序列化结果

**事件流** (后端推送): FileWatcher → `events::emit_xxx()` → Tauri event → `api.onXxx` 监听器 → 防抖刷新 store → React 重渲染

**SSE 桥接** (HTTP 模式): 后端事件 → `SSEBroadcaster` (tokio broadcast) → HTTP SSE → 前端 httpClient 接收

**错误检测流** (后台管道): FileWatcher → JSONL 解析 → ErrorDetector → NotificationManager → 持久化 + 原生通知

## Key Design Patterns

- **双传输**: `ElectronAPI` 接口作为统一契约，通过 `window.__TAURI_INTERNALS__` 懒选择 TauriIPC 或 HTTP+SSE
- **Per-Tab UI 隔离**: `tabUISlice` 为每个标签页维护独立的展开/滚动状态
- **Config 深度合并**: 加载时与默认值合并，新增字段自动填充；分区更新保留未变更字段
- **Rust 命令返回**: `Result<T, String>`，错误为序列化字符串
- **数据源**: 文件系统 (`~/.claude/projects/{hash}/*.jsonl`) 为唯一真实来源
