# AGENTS.md

This file provides guidance to Codex (Codex.ai/code) when working with code in this repository.

基于 Tauri v2 的 Codex 会话可视化桌面应用。读取 `~/.Codex/projects/` 下的 JSONL 会话文件，解析为结构化数据，提供对话浏览、上下文追踪、工具调用分析和错误检测通知等功能。支持本地和 SSH 远程连接，以及内置 HTTP 服务器模式（浏览器访问）。

## Quick Reference

```bash
pnpm install            # 安装依赖
pnpm tauri dev          # 前端 Vite (5173) + Rust 后端同时启动
pnpm build              # 前端 TS 编译 + Vite 打包
pnpm build:macos        # 完整 Tauri 构建 (DMG)
pnpm lint               # ESLint

# Rust 测试
cd src-tauri && cargo test                          # 全部测试
cd src-tauri && cargo test -- <module_name>         # 单模块过滤（如 config, ssh_connection）
cd src-tauri && cargo test -p Codex-devtools -- <name>  # 按名称过滤

# 前端测试（vitest + happy-dom）
npx vitest run
npx vitest run -- path/to/test.test.tsx
```

## 关键覆盖规则

- **Tauri API**: `withGlobalTauri: false`，必须从 `@tauri-apps/api` 直接导入，不可使用全局变量
- **路径别名**: `@main/*` → `src/main/*`, `@renderer/*` → `src/*`, `@shared/*` → `src/shared/*`
- **Rust 命令**: snake_case 命名，返回 `Result<T, String>`
- **双传输层**: 新增 API 时需同时实现 `TauriAPIClient` 和 `HttpAPIClient`（通过 `ElectronAPI` 接口统一契约，定义在 `src/shared/types/api.ts`）
- **ReDoS 防护**: `trigger_manager/` 下的正则必须通过 `regex_validation.rs` 检查
- **唯一数据源**: 文件系统 (`~/.Codex/projects/{hash}/*.jsonl`) 为唯一真实来源

## 架构概览

### 双模式运行

应用有两种运行模式，通过 `src/api/index.ts` 的 Proxy 自动切换：
- **Tauri 桌面模式**: `window.__TAURI_INTERNALS__` 存在时使用 `TauriAPIClient`（IPC invoke）
- **浏览器 HTTP 模式**: 否则使用 `HttpAPIClient`（REST + SSE），由 `src-tauri/src/http/` 的 Axum 服务器提供

前端代码只需 `import { api } from "@renderer/api"` 即可，不感知底层传输方式。

### Rust 后端分层 (`src-tauri/src/`)

```
commands/          → Tauri IPC 命令处理（thin layer，调用 services）
services/          → 业务逻辑层（trait + impl 分离）
  ├── *_trait.rs   → trait 定义
  └── *_impl.rs    → 实现（ProjectService, SessionService, SearchService, SshService, ConfigService, SubagentService）
infrastructure/    → 基础设施
  ├── fs_provider/ → FsProvider trait（本地 LocalFsProvider + SSH SshFsProvider）
  ├── context_manager/  → 多上下文管理（local / ssh），ServiceContext 封装单工作空间
  ├── service_context/  → 单上下文服务栈（scanner + searcher + watcher + cache）
  ├── config/      → ConfigManager（JSON 配置读写、深度合并）
  ├── file_watcher/ → 文件变更监听（notify crate）
  ├── data_cache/  → moka LRU 缓存
  ├── ssh_connection/ → SSH 连接管理（russh）
  ├── notification/ → 通知管理（CRUD + 持久化 + 触发匹配）
  ├── trigger_manager/ → 错误触发器（正则匹配 + 验证）
  └── app_bootstrap/ → 应用启动编排
parsing/           → JSONL 解析、消息分类、tool 提取、claude_md 读取
analysis/          → 会话分析（chunk 构建、对话分组、waterfall、semantic steps）
discovery/         → 项目发现、会话搜索、subagent 解析、worktree 分组
types/             → 核心类型（jsonl, domain, messages, chunks, config, ssh）
error/             → 错误检测 + 触发匹配
http/              → Axum HTTP 服务器（routes/ 镜像 commands/ 的功能，SSE 事件流）
utils/             → 工具函数（分页、路径解码、正则校验、时间处理）
```

**关键设计**: commands 薄层委托 services；services 通过 `Arc<dyn Trait>` 注入，在 `lib.rs::setup()` 中注册为 Tauri managed state。HTTP routes 共享同一个 service 实例。

### 前端 (`src/`)

- **API 层**: `src/api/` — `ElectronAPI` 接口的两种实现（Tauri / HTTP），Proxy 懒加载
- **状态管理**: `src/store/` — Zustand slices 模式（14 个 slice），`initializeNotificationListeners()` 启动事件订阅
- **UI 组件**: `src/components/` — chat（对话展示）、sidebar、search、settings、notifications、dashboard、layout
- **工具函数**: `src/utils/` — contextTracker、claudeMdTracker、toolLinkingEngine、displayItemBuilder
- **共享层**: `src/shared/` — 跨进程类型定义和纯工具函数

### 数据流

```
~/.Codex/projects/{hash}/*.jsonl
  → FileWatcher (notify crate) 检测变更
  → ContextManager 管理 ServiceContext 生命周期
  → Tauri events (file-change, todo-change, notification:*, ssh:status) 或 SSE 推送到前端
  → Zustand store 更新 → React 重渲染
```

### 事件通道

| 事件名 | 用途 |
|--------|------|
| `file-change` | 文件变更（新会话、会话内容更新） |
| `todo-change` | 任务列表变更 |
| `notification:new` / `notification:updated` | 错误检测通知 |
| `notification:clicked` | 原生通知点击导航 |
| `ssh:status` | SSH 连接状态变更 |
| `tray:open-session` | 系统托盘打开会话 |
| `session:refresh` | Cmd+R 强制刷新 |

### 关键模式

- **FsProvider trait**: 抽象文件系统操作（本地 `std::fs` vs SSH/SFTP），使 discovery/services 不感知数据来源
- **多上下文**: ContextManager 管理多个 ServiceContext（local + SSH），切换时停/启 watcher
- **双传输层**: 同一个 service 实例同时服务 Tauri IPC 和 Axum HTTP；HTTP routes 在 `http/routes/` 下逐一对应 commands
- **自适应刷新**: store 根据会话大小动态调整 file-change 事件的 debounce 间隔（150ms ~ 60s）
- **Per-Tab UI 隔离**: `tabUISlice` 按 tabId 维护独立的展开状态，多 tab 互不干扰

<!-- gitnexus:start -->
# GitNexus — Code Intelligence

This project is indexed by GitNexus as **claude-devtools-tauri** (8749 symbols, 16552 relationships, 300 execution flows). Use the GitNexus MCP tools to understand code, assess impact, and navigate safely.

> If any GitNexus tool warns the index is stale, run `npx gitnexus analyze` in terminal first.

## Always Do

- **MUST run impact analysis before editing any symbol.** Before modifying a function, class, or method, run `gitnexus_impact({target: "symbolName", direction: "upstream"})` and report the blast radius (direct callers, affected processes, risk level) to the user.
- **MUST run `gitnexus_detect_changes()` before committing** to verify your changes only affect expected symbols and execution flows.
- **MUST warn the user** if impact analysis returns HIGH or CRITICAL risk before proceeding with edits.
- When exploring unfamiliar code, use `gitnexus_query({query: "concept"})` to find execution flows instead of grepping. It returns process-grouped results ranked by relevance.
- When you need full context on a specific symbol — callers, callees, which execution flows it participates in — use `gitnexus_context({name: "symbolName"})`.

## Never Do

- NEVER edit a function, class, or method without first running `gitnexus_impact` on it.
- NEVER ignore HIGH or CRITICAL risk warnings from impact analysis.
- NEVER rename symbols with find-and-replace — use `gitnexus_rename` which understands the call graph.
- NEVER commit changes without running `gitnexus_detect_changes()` to check affected scope.

## Resources

| Resource | Use for |
|----------|---------|
| `gitnexus://repo/claude-devtools-tauri/context` | Codebase overview, check index freshness |
| `gitnexus://repo/claude-devtools-tauri/clusters` | All functional areas |
| `gitnexus://repo/claude-devtools-tauri/processes` | All execution flows |
| `gitnexus://repo/claude-devtools-tauri/process/{name}` | Step-by-step execution trace |

## CLI

| Task | Read this skill file |
|------|---------------------|
| Understand architecture / "How does X work?" | `.claude/skills/gitnexus/gitnexus-exploring/SKILL.md` |
| Blast radius / "What breaks if I change X?" | `.claude/skills/gitnexus/gitnexus-impact-analysis/SKILL.md` |
| Trace bugs / "Why is X failing?" | `.claude/skills/gitnexus/gitnexus-debugging/SKILL.md` |
| Rename / extract / split / refactor | `.claude/skills/gitnexus/gitnexus-refactoring/SKILL.md` |
| Tools, resources, schema reference | `.claude/skills/gitnexus/gitnexus-guide/SKILL.md` |
| Index, status, clean, wiki CLI commands | `.claude/skills/gitnexus/gitnexus-cli/SKILL.md` |

<!-- gitnexus:end -->
