# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 项目概述

基于 Tauri v2 的 Claude Code 会话可视化桌面应用。读取 `~/.claude/projects/` 下的 JSONL 会话文件，解析为结构化数据，提供对话浏览、上下文追踪、工具调用分析和错误检测通知等功能。从 [claude-devtools](https://github.com/matt1398/claude-devtools) (Electron) 移植而来。

## 常用命令

```bash
# 开发
pnpm install
pnpm tauri dev          # 前端 Vite (5173) + Rust 后端同时启动

# 构建
pnpm build              # 仅前端 TypeScript 编译 + Vite 打包
pnpm build:macos        # 完整 Tauri 构建 (DMG)

# 代码检查
pnpm lint               # ESLint

# Rust 测试
cd src-tauri && cargo test                    # 全部测试
cd src-tauri && cargo test -- config_manager  # 单模块测试
cd src-tauri && cargo test -p claude-devtools-tauri -- chunk_builder  # 按名称过滤

# 前端测试（vitest 已安装但尚未编写测试文件）
npx vitest run
npx vitest run -- path/to/test.test.ts
```

## 技术栈

- **后端**: Rust + Tauri v2.10, tokio (异步运行时), serde, notify (文件监听), moka (LRU 缓存)
- **前端**: React 19, TypeScript, Vite 8, Tailwind CSS 3, Zustand 5
- **关键依赖**: @tanstack/react-virtual (虚拟滚动), @dnd-kit (标签拖拽), react-markdown + remark-gfm

## 架构

### 后端模块 (`src-tauri/src/`)

```
lib.rs              # 入口，插件注册，55+ Tauri commands 通过 generate_handler! 注册
├── commands/       # IPC 命令处理 (sessions, projects, search, config, notifications, updater 等)
├── types/          # 类型定义
│   ├── jsonl.rs    # ChatHistoryEntry 枚举，ContentBlock (Text/Thinking/ToolUse/ToolResult/Image)
│   ├── domain.rs   # Session, Project, IpcResult<T>, PaginatedSessionsResult
│   ├── chunks.rs   # Chunk 枚举 (User/Ai/System/Compact)，AiChunk 含 ToolExecution/SemanticStep/Process
│   └── config.rs   # AppConfig, NotificationTrigger, StoredNotification
├── parsing/        # JSONL 解析管道
│   ├── jsonl_parser.rs         # 逐行读取 JSONL → ChatHistoryEntry
│   ├── message_classifier.rs   # 消息分类 → MessageCategory (User/System/HardNoise/Ai/Compact)
│   └── session_parser.rs       # 编排解析，产出 ParsedSession
├── analysis/       # 分析管道
│   ├── chunk_builder.rs            # 消息 → 可视化 Chunk（核心）
│   ├── tool_execution_builder.rs   # 配对 tool_use ↔ tool_result
│   ├── semantic_step_extractor.rs  # 从 AI 块提取语义步骤
│   ├── process_linker.rs           # 关联子 Agent 进程到父 AI 块
│   └── waterfall_builder.rs        # 瀑布图数据构建
├── discovery/      # 项目扫描、会话搜索、子 Agent 解析
├── error/          # 错误检测管道 (ErrorDetector → ErrorTriggerChecker → NotificationManager)
├── infrastructure/ # 基础设施
│   ├── config_manager.rs          # ~/.claude/claude-devtools-config.json 读写，深度合并默认值
│   ├── data_cache.rs              # moka LRU 缓存 (50 条, 10 min TTL)，版本化失效
│   ├── file_watcher.rs            # notify-debouncer-mini (100ms debounce)，broadcast 事件
│   └── notification_manager.rs    # 通知持久化，原生通知节流 (5s/唯一 hash)，snooze/ignore
└── events.rs        # 后端 → 前端事件: file-change, todo-change, notification:new/updated, error:detected
```

**共享状态**: `Arc<RwLock<AppState>>` (cache + config_manager)，通过 `State<'_>` 注入到 Tauri commands。

**三个并发文件监听器**: 主监听器 (JSONL/JSON)、错误检测管道 (仅非子 Agent JSONL)、Todo 监听器。

### 前端结构 (`src/`)

```
api/                  # 双传输层抽象
│   ├── index.ts      # Proxy 懒加载：Tauri (invoke/listen) vs HTTP (fetch/SSE)
│   ├── tauriClient.ts
│   └── httpClient.ts
store/                # Zustand 14 切片模式
│   ├── index.ts      # Store 创建 + IPC 事件监听初始化（file-change 防抖刷新等）
│   └── slices/       # project, session, sessionDetail, tab, tabUI, notification, config 等
components/
├── chat/             # 会话展示：AIChatGroup, UserChatGroup, DisplayItem, linkedTool viewers
├── chat/SessionContextPanel/  # 上下文面板：CLAUDE.md 注入、文件提及、工具输出、thinking
├── layout/           # TabbedLayout, PaneContainer, PaneView (分栏), TabBar (拖拽)
├── sidebar/          # 会话列表，日期分组
├── settings/         # 设置 UI + NotificationTriggerSettings (复杂触发器配置)
└── search/           # CommandPalette (跨项目), SearchBar (会话内)
```

**三层类型系统**: `src/main/types/` (镜像 Rust) → `src/shared/types/` (跨进程契约) → `src/types/` (渲染器专用)

**路径别名**: `@main/*` → `src/main/*`, `@renderer/*` → `src/*`, `@shared/*` → `src/shared/*`

### 前后端通信

**命令流** (请求-响应): React → Zustand action → `api.xxx()` → `TauriAPIClient.invoke('xxx')` → Rust command → 返回序列化结果

**事件流** (后端推送): FileWatcher → `events::emit_xxx()` → Tauri event → `api.onXxx` 监听器 → 防抖刷新 store → React 重渲染

**错误检测流** (后台管道): FileWatcher → JSONL 解析 → ErrorDetector → NotificationManager → 持久化 + 原生通知

### 关键设计模式

- **双传输**: `ElectronAPI` 接口作为统一契约，通过 `window.__TAURI_INTERNALS__` 懒选择 TauriIPC 或 HTTP+SSE
- **Per-Tab UI 隔离**: `tabUISlice` 为每个标签页维护独立的展开/滚动状态
- **Config 深度合并**: 加载时与默认值合并，新增字段自动填充；分区更新保留未变更字段
- **Rust 命令返回**: `Result<T, String>`，错误为序列化字符串
- **数据源**: 文件系统 (`~/.claude/projects/{hash}/*.jsonl`) 为唯一真实来源

## 开发注意事项

- Tauri 命令名称使用 snake_case，前端调用时通过 `invoke` 映射
- 前端组件使用 `@tanstack/react-virtual` 处理大量会话/消息的虚拟滚动
- macOS 私有 API 已启用（Dock 隐藏），使用 `cocoa` + `objc` crate
- 窗口使用 Overlay 标题栏样式，traffic lights 位于 (16, 18)
- 前端测试基础设施已就绪 (vitest + happy-dom + @testing-library/react) 但尚未编写测试用例
