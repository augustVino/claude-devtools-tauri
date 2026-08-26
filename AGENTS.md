# AGENTS.md

This file provides guidance to AI coding agents when working with code in this repository.

基于 Tauri v2 的**多 agent 会话可视化桌面应用**。读取本地（或 SSH 远端）多家 coding agent 工具的会话记录，解析为统一的 Claude 语义中间表示，提供对话浏览、上下文追踪、工具调用分析、错误检测通知等功能。支持本地和 SSH 远程连接，以及内置 HTTP 服务器模式（浏览器访问）。

**支持的 agent 工具**（每家一个 adapter，见 `src-tauri/src/agents/`）：

| Agent | 数据源 | 类型 | 实时刷新 | SSH |
|---|---|---|---|---|
| Claude Code | `~/.claude/projects/{编码目录}/*.jsonl` | JSONL | ✅ notify + 轮询 | ✅ |
| Codex CLI | `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`（+archived_sessions） | JSONL 日期树 | ✅ notify（SSH 拉模式） | ✅ |
| OpenCode | `~/.local/share/opencode/opencode.db`（XDG/OPENCODE_DB 候选） | SQLite（只读） | TTL 兜底 | ❌ 本地 only |
| Pi | `~/.pi/agent/sessions/{编码目录}/{ts}_{uuid}.jsonl` | JSONL | ✅ notify + 轮询 | ✅ |
| dsh | `~/.dsh/sessions/{dir}/{id}/session.jsonl[.zstd]` | zstd 多帧 JSONL | ✅ 全量重解 | ✅ |

## Quick Reference

```bash
pnpm install            # 安装依赖
pnpm tauri dev          # 前端 Vite (5173) + Rust 后端同时启动
pnpm build              # 前端 TS 编译 + Vite 打包
pnpm build:macos        # 完整 Tauri 构建 (DMG)
pnpm lint               # ESLint

# Rust 测试
cd src-tauri && cargo test                          # 全部测试
cd src-tauri && cargo test --lib agents::           # 单模块过滤（如 agents, config, ssh_connection）
cd src-tauri && cargo test --lib -- --ignored       # 真实数据 smoke（依赖本机 agent 数据）

# 前端类型检查（前端目前无测试文件，tsc 即验证边界）
npx tsc --noEmit
```

## 关键覆盖规则

- **Tauri API**: `withGlobalTauri: false`，必须从 `@tauri-apps/api` 直接导入，不可使用全局变量
- **路径别名**: `@main/*` → `src/main/*`, `@renderer/*` → `src/*`, `@shared/*` → `src/shared/*`
- **Rust 命令**: snake_case 命名，返回 `Result<T, String>`
- **双传输层**: 新增 API 时需同时实现 `TauriAPIClient` 和 `HttpAPIClient`（通过 `ElectronAPI` 接口统一契约，定义在 `src/shared/types/api.ts`）
- **ReDoS 防护**: `trigger_manager/` 下的正则必须通过 `regex_validation.rs` 检查
- **唯一数据源**: 各 agent 的本地数据文件是唯一真实来源；应用全程只读（OpenCode 以 `SQLITE_OPEN_READ_ONLY` 打开；删除走系统 trash）

## 多 Agent 架构（核心，见 `src-tauri/src/agents/mod.rs` 模块文档）

### 防腐层（Anti-Corruption Layer）

各家格式知识收敛在各自 adapter（`claude.rs` / `codex.rs` / `opencode.rs` / `pi.rs` / `dsh.rs`）内，向下游（分类、chunk 构建、瀑布图、前端渲染）只暴露统一的 `ParsedMessage` 中间表示 —— 下游管线不感知会话来自哪家 agent。契约分三类字段（详见模块 rustdoc）：

1. **中立字段**（直接映射）：uuid/timestamp/role/model/usage/content 四种块（`text`/`thinking`/`tool_use`/`tool_result`，即 Anthropic 块协议作为通用语）/cwd/git_branch
2. **泛化语义字段**（各家用自家证据填充）：`is_meta`（非真人输入：Claude isMeta / Codex 前缀清单 / OpenCode synthetic / dsh source.kind）、`is_compact_summary`、`is_sidechain`
3. **Claude 特有字段**（他家输出 None，下游遇空短路）：`request_id`（含去重，收编在 ClaudeAdapter 内）等

### 新增 agent 的检查单

1. `types/domain.rs` 的 `AgentKind` 加变体（`ALL` 顺序 = 前端展示序）
2. 新建 `agents/<name>.rs` 实现 `AgentAdapter` —— **在 parse 阶段消化自家噪声**，不得依赖下游 classifier 认识你的原生格式
3. 实现**聚合协议全部方法**（`data_root_under` / `scan_sessions` / `locate_session` / `light_session`），缺一个就会"项目有会话数但列表为空"
4. `create_adapters()` 注册 + `owns_path` 结构特征判定
5. 不认识的行 `log::warn!`（schema 漂移金丝雀）

### 跨 agent 项目归并 —— cwd 权威路径匹配（不要用 id 同构！）

Claude 的目录名编码规则与其 CLI 版本相关（`.` 等字符也编码为 `-`），与 `encode_path` **必然分叉**。归并唯一依据是 cwd 路径相等：Claude 侧 `Project.path` 来自会话内 cwd（权威），其他 agent 侧来自文件内容（pi/codex/dsh 首行或 OpenCode directory 列）。自编码 id 仅保证本模块内部一致（list/locate 复用同函数）。

### 性能架构（SSH 高延迟链路实测教训）

三层缓存 + 并行 + 单飞，全部**事件驱动失效**（TTL 只兜底）：

| 层 | 位置 | 挡住的 IO |
|---|---|---|
| 聚合缓存 | `agents/mod.rs` AggCache | extra agent 树扫描 / light preview / 根存在性（含**负缓存**） |
| listing 缓存 | `infrastructure/listing_cache.rs` | claude 项目/会话列表全量重扫 |
| git facts 缓存 | `infrastructure/git_facts_cache.rs` | worktree 分组的 6 项 git 身份解析 |

- **`agents::par_map`**（worker pool）：SSH 逐文件头读并行化。**锁纪律**：fs IO 绝不在 Mutex 内执行（被违反过一次导致 8 并发退化为串行，有并发性回归测试守着）
- **单飞锁**（`PROJECTS_SCAN_FLIGHT` 等）：并发调用只有一个真扫，等锁者 double-check 缓存
- **SSH read_file_head 阶梯式**：首读 8KB，行数不足才升 64KB
- 所有列表链路有 `[perf]` 日志（START/END/cache HIT），排查慢先看日志再动手
- OpenCode 寻址用虚拟路径 `{db}#{session_id}` —— `session_service::path_exists` 已感知，新代码判存在必须走它

## 架构概览

### 双模式运行

应用有两种运行模式，通过 `src/api/index.ts` 的 Proxy 自动切换：
- **Tauri 桌面模式**: `window.__TAURI_INTERNALS__` 存在时使用 `TauriAPIClient`（IPC invoke）
- **浏览器 HTTP 模式**: 否则使用 `HttpAPIClient`（REST + SSE），由 `src-tauri/src/http/` 的 Axum 服务器提供

前端代码只需 `import { api } from "@renderer/api"` 即可，不感知底层传输方式。

### Rust 后端分层 (`src-tauri/src/`)

```
agents/            → 多 agent 防腐层（AgentAdapter trait + 各家实现 + 聚合/缓存/par_map）
commands/          → Tauri IPC 命令处理（thin layer，调用 services）
services/          → 业务逻辑层（trait + impl 分离）
  ├── *_trait.rs   → trait 定义
  └── *_impl.rs    → 实现（ProjectService, SessionService, SearchService, SshService, ConfigService, SubagentService）
infrastructure/    → 基础设施
  ├── fs_provider/ → FsProvider trait（本地 LocalFsProvider + SSH SshFsProvider）
  ├── context_manager/  → 多上下文管理（local / ssh），ServiceContext 封装单工作空间
  ├── service_context/  → 单上下文服务栈（scanner + searcher + watcher + cache + home_dir）
  ├── listing_cache.rs  → claude 列表结果缓存（TTL + 事件失效）
  ├── git_facts_cache.rs → worktree 分组 git 身份缓存
  ├── config/      → ConfigManager（JSON 配置读写、深度合并）
  ├── file_watcher/ → 多根文件监听（claude 主根 + extra agent 根；local notify / SSH 轮询）
  ├── data_cache/  → moka LRU 缓存
  ├── ssh_connection/ → SSH 连接管理（russh）
  ├── notification/ → 通知管理（CRUD + 持久化 + 触发匹配）
  ├── trigger_manager/ → 错误触发器（正则匹配 + 验证）
  └── app_bootstrap/ → 应用启动编排
parsing/           → JSONL 解析、消息分类（claude 语义）、tool 提取、claude_md 读取
analysis/          → 会话分析（chunk 构建、对话分组、waterfall、semantic steps）—— 作用于统一 ParsedMessage
discovery/         → claude 项目发现、会话搜索（含 extra agent 追加）、worktree 分组
types/             → 核心类型（jsonl, domain（AgentKind/Session.agent）, messages, chunks, config, ssh）
error/             → 错误检测 + 触发匹配（claude 语义，extra agent 不参与）
http/              → Axum HTTP 服务器（routes/ 镜像 commands/ 的功能，SSE 事件流）
utils/             → 工具函数（分页、路径解码、时间处理）
```

**关键设计**: commands 薄层委托 services；services 通过 `Arc<dyn Trait>` 注入，在 `lib.rs::setup()` 中注册为 Tauri managed state。HTTP routes 共享同一个 service 实例。

### 前端 (`src/`)

- **API 层**: `src/api/` — `ElectronAPI` 接口的两种实现（Tauri / HTTP），Proxy 懒加载
- **状态管理**: `src/store/` — Zustand slices 模式，`initializeNotificationListeners()` 启动事件订阅
- **UI 组件**: `src/components/` — chat（对话展示，AI 组标题按 `session.agent` 显示 agent 名）、sidebar（会话行带 agent 徽标，居右）、search、settings、notifications、dashboard、layout
- **工具函数**: `src/utils/` — contextTracker、claudeMdTracker、toolLinkingEngine、displayItemBuilder
- **共享层**: `src/shared/` — 跨进程类型定义和纯工具函数（`AgentKind` TS 类型在 `src/main/types/domain.ts`）

### 数据流

```
各 agent 数据源（本地或 SSH 远端）
  → AgentAdapter（parse → 统一 ParsedMessage；scan/locate → AgentSessionEntry）
  → Claude 既有管线（分类 / chunk / 分析）+ 聚合层（项目归并按 cwd）
  → FileWatcher 多根监听（notify / SSH 3s 轮询）
  → 缓存失效（listing/agg/detail fingerprint）→ Tauri events 或 SSE
  → Zustand store → React 重渲染
```

### 事件通道

| 事件名 | 用途 |
|--------|------|
| `file-change` | 文件变更（含 `agent` 字段标记 extra agent 来源；extra 事件的 projectId 发送前置空） |
| `todo-change` | 任务列表变更（claude） |
| `notification:new` / `notification:updated` | 错误检测通知（claude 会话） |
| `notification:clicked` | 原生通知点击导航 |
| `ssh:status` | SSH 连接状态变更 |
| `tray:open-session` | 系统托盘打开会话 |
| `session:refresh` | Cmd+R 强制刷新 |

### 关键模式

- **FsProvider trait**: 抽象文件系统操作（本地 `std::fs` vs SSH/SFTP），adapter 的所有读取必须走 provider（SSH 模式下本地 fs 读不到远端文件）
- **多上下文**: ContextManager 管理多个 ServiceContext（local + SSH）；`home_dir` 是 extra agent 数据根的推导基准（SSH 从 projects_dir 的 `.claude/projects` 组件序列推导，`Some(空)` 会短路推导分支 —— 曾因此导致 SSH 看不到 extra agent）
- **双传输层**: 同一个 service 实例同时服务 Tauri IPC 和 Axum HTTP
- **自适应刷新**: store 根据会话大小动态调整 file-change 事件的 debounce 间隔（150ms ~ 60s）
- **Per-Tab UI 隔离**: `tabUISlice` 按 tabId 维护独立的展开状态，多 tab 互不干扰

### 能力边界（有意取舍，非欠账）

- OpenCode：无实时刷新（TTL 兜底）、SSH 不可用（SQLite 需本地随机读）、删除 no-op（只读原则）
- Codex：SSH 模式无实时刷新（日期树轮询成本）；`state_5.sqlite` 手动标题未接入
- 错误检测通知仅覆盖 Claude 会话（各家错误语义未定义）
- pi 子代理（`custom:subagents:record`）未解析，hasSubagents 恒 false

<!-- gitnexus:start -->
# GitNexus — Code Intelligence

This project is indexed by GitNexus as **claude-devtools-tauri** (9148 symbols, 17612 relationships, 300 execution flows). Use the GitNexus MCP tools to understand code, assess impact, and navigate safely.

> If any GitNexus tool warns the index is stale, run `npx gitnexus analyze` in terminal first.

## Always Do

- **MUST run impact analysis before editing any symbol.** Before modifying a function, class, or method, run `gitnexus_impact({target: "symbolName", direction: "upstream"})` and report the blast radius (direct callers, affected processes, risk level) to the user.
- **MUST run `gitnexus_detect_changes()` before committing** to verify your changes only affect expected symbols and execution flows.
- **MUST warn the user** if impact analysis returns HIGH or CRITICAL risk before proceeding with edits.
- **When exploring unfamiliar code** | use `gitnexus_query({query: "concept"})` to find execution flows instead of grepping. It returns process-grouped results ranked by relevance.
- **When you need full context on a specific symbol** — callers, callees, which execution flows it participates in, use `gitnexus_context({name: "symbolName"})`.

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
