# openInLauncher 子系统迁移设计

> 日期：2026-05-30
> 最后修订：2026-05-30（根据深度代码审查修复）
> 上游来源：`claude-devtools/src/main/utils/openInLauncher.ts` + `src/main/ipc/memory.ts`
> 上游前端：`src/renderer/components/sidebar/memory/OpenInMenu.tsx`、`MemoryEntryPreview.tsx`、`MemoryView.tsx`（工具栏部分）

## 概述

将 Electron 的 openInLauncher 子系统迁移到 Tauri，实现 macOS 下 Memory 文件/目录"用指定应用打开"功能。仅支持 macOS，检测 10 个应用：Finder、Cursor、VS Code、Zed、Xcode、Ghostty、iTerm、Terminal、Android Studio、Antigravity。

## 架构

```
前端 (React)                              Rust 后端
┌──────────────┐                          ┌────────────────────┐
│ OpenInMenu   │                          │ commands/memory.rs │
│ MemorySection│ ◄── api.memory ─────────►│ list_memory_openers│
│ MemoryView   │     openIn()             │ open_memory_in     │
└──────────────┘                          └────────┬───────────┘
                                                   │
                                     ┌─────────────▼──────────────┐
                                     │ services/memory_service.rs │
                                     │ list_available_openers()   │
                                     │ open_in()                  │
                                     └─────────────┬──────────────┘
                                                   │
                                     ┌─────────────▼──────────────┐
                                     │ services/app_opener.rs     │
                                     │ (新模块)                    │
                                     │ detect_installations()     │
                                     │ open_with()                │
                                     │ (10 个 macOS 检测器)        │
                                     └────────────────────────────┘
```

## 后端设计

### `services/app_opener.rs`（新模块）

**放置位置**：新建 `services/app_opener.rs` 而非 `parsing/`。该模块涉及系统进程调用（`mdfind`、`open`）和应用检测逻辑，属于系统集成功能，与 `parsing/`（JSONL 解析、消息分类等纯数据转换）职责不符。与 `git_identity.rs` 一样是功能性服务，但放在 `services/` 更符合系统操作类模块的归类。

#### 数据结构

```rust
/// 可用来打开 Memory 文件/目录的外部应用目标。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenTarget {
    pub id: String,       // "finder", "vscode", "cursor" ...
    pub name: String,     // "Finder", "VS Code", "Cursor" ...
    pub icon_name: String, // lucide icon name，用于前端图标渲染
    pub available: bool,  // 当前系统是否已安装该应用
}

/// opener_id 白名单枚举。命令层解析传入的 opener_id 字符串，
/// 未知值返回 AppError::InvalidInput。
pub enum OpenTargetId {
    Finder, Cursor, VsCode, Zed, Xcode,
    Ghostty, ITerm, Terminal, AndroidStudio, Antigravity,
}

impl std::str::FromStr for OpenTargetId { /* ... */ }
```

#### 检测策略

| 应用 | 检测方式 | 打开方式 |
|------|----------|----------|
| Finder | 始终可用 | `open <path>`（目录）/ `open -R <path>`（文件选中） |
| VS Code | `mdfind "kMDItemFSName == 'Visual Studio Code.app'"`，fallback 检查 `/Applications/Visual Studio Code.app` 和 `~/Applications/Visual Studio Code.app` | `open -a "Visual Studio Code" <path>` |
| Cursor | `mdfind "kMDItemFSName == 'Cursor.app'"`，fallback 检查 `/Applications/Cursor.app` | `open -a "Cursor" <path>` |
| Zed | `mdfind "kMDItemFSName == 'Zed.app'"`，fallback 检查 `/Applications/Zed.app` | `open -a "Zed" <path>` |
| Xcode | `mdfind "kMDItemFSName == 'Xcode.app'"`，fallback 检查 `/Applications/Xcode.app` | `open -a "Xcode" <path>` |
| Ghostty | `mdfind "kMDItemFSName == 'Ghostty.app'"`，fallback 检查 `/Applications/Ghostty.app` | `open -a "Ghostty" <path>` |
| iTerm | `mdfind "kMDItemFSName == 'iTerm.app'"`，fallback 检查 `/Applications/iTerm.app` | `open -a "iTerm" <path>` |
| Terminal | 检查 `/System/Applications/Utilities/Terminal.app` | `open -a "Terminal" <path>` |
| Android Studio | `mdfind "kMDItemFSName == 'Android Studio.app'"`，fallback 检查 `/Applications/Android Studio.app` | `open -a "Android Studio" <path>` |
| Antigravity | `mdfind "kMDItemFSName == 'Antigravity.app'"`，fallback 检查 `/Applications/Antigravity.app` | `open -a "Antigravity" <path>` |

**Fallback 策略**：`mdfind` 失败或未找到时，使用 `std::fs::metadata` 检查 `/Applications/{name}.app` 和 `~/Applications/{name}.app` 是否存在。不使用 `lsregister`（输出量巨大、解析脆弱、速度慢）。

#### 异步与超时

- 检测函数使用 `tokio::process::Command` + `tokio::time::timeout(Duration::from_secs(2))`，每个应用的 mdfind 调用独立 2s 超时
- `open -a` 操作使用 `tokio::process::Command` 异步执行，不阻塞 async runtime
- 不使用 `std::process::Command`（会阻塞 tokio worker 线程）

#### 安全

- 不使用 `sh -c` 包裹命令，所有参数直接传给 `Command::new("mdfind")` / `Command::new("open")`
- `OpenTargetId::from_str` 作为白名单：无法解析的 opener_id 返回 `Err`

### `services/memory_service.rs` — trait 扩展

在 `MemoryService` trait 中新增：

```rust
async fn list_available_openers(&self) -> Result<Vec<OpenTarget>, AppError>;
async fn open_in(
    &self,
    opener_id: &str,
    project_id: &str,
    file_name: Option<&str>,
) -> Result<(), AppError>;
```

在 `MemoryServiceImpl` 中的实现逻辑：
- `list_available_openers`：直接委托 `super::app_opener::detect_installations()`
- `open_in`：解析 `opener_id` → `OpenTargetId`（白名单校验），通过 `get_dir_path`/`get_file_path` 获取目标路径，委托 `super::app_opener::open_with(opener_id, &path, is_directory)`

### `error/app_error.rs` — 新增变体

```rust
OpenFailed(String),
```

用于 `open -a` 命令失败、应用未安装等场景。与 `FileOp` 区分：`FileOp` 用于文件读写操作失败，`OpenFailed` 用于外部应用打开失败。

### `commands/memory.rs` — 新增 2 命令

在现有 4 个命令后追加：

```rust
#[command]
pub async fn list_memory_openers(
    service: State<'_, Arc<dyn MemoryService>>,
) -> Result<Vec<OpenTarget>, String>;

#[command]
pub async fn open_memory_in(
    service: State<'_, Arc<dyn MemoryService>>,
    project_id: String,
    file_name: Option<String>,
    opener_id: String,
) -> Result<MemoryOpenResult, String>;
```

### `http/routes/memory.rs` — 新增 2 端点

- `GET /api/memory/openers` → `Json<Vec<OpenTarget>>`
- `POST /api/memory/open-in` → `Json<MemoryOpenResult>`

新增 `OpenInBody` struct（注意：不是 `OpenInQuery`，"Query" 暗示 URL query params，与现有 `MemoryFileQuery` 模式区分）：

```rust
#[derive(Deserialize)]
pub struct OpenInBody {
    pub project_id: String,
    #[serde(default)]
    pub file_name: Option<String>,
    pub opener_id: String,
}
```

**HTTP 模式限制**：HTTP 模式下 `open_in` 返回 `success: false, error: Some("Open operations are not available in HTTP mode")`。与上游 Electron 的 `openInEditor: console.warn('not available in browser mode')` 行为一致。

### `types/memory.rs` — 扩展

```rust
/// 打开文件/目录的操作结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryOpenResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
```

`OpenTarget` 也定义在 `types/memory.rs`，带 `#[serde(rename_all = "camelCase")]`。

### `lib.rs` — 注册新命令

在 `invoke_handler!` 中现有 memory 命令（`commands::memory::copy_memory_path`）之后追加：

```rust
commands::memory::list_memory_openers,
commands::memory::open_memory_in,
```

### `services/mod.rs` — 注册新模块

```rust
pub mod app_opener;
```

### 单元测试

- `OpenTargetId::from_str`：10 个已知变体 + 1 个未知值拒绝
- `OpenTarget` 序列化：验证 camelCase 输出
- `MemoryOpenResult` 序列化：验证 `success` 为 boolean 非 string
- `detect_installations`：仅 macOS 平台运行（`#[cfg(target_os = "macos")]`）

## 前端设计

### `OpenInMenu.tsx`（新建）

- 位置：`src/components/sidebar/memory/OpenInMenu.tsx`
- **不使用 `Command` 组件**（该项目不存在）。跟随现有 `SessionContextMenu.tsx` 的 custom dropdown 模式：
  - `fixed`/`absolute` 定位 + viewport clamping
  - `pointerdown` 外部点击关闭（`document.addEventListener`）
  - `Escape` 键关闭
  - Tailwind + CSS 变量样式（`var(--color-surface-overlay)`、`var(--color-border-emphasis)`）
- 两个变体：`'dots'`（侧边栏 `⋯` 按钮）和 `'iconMenu'`（工具栏图标样式）
- 点击时调用 `api.memory.openIn(projectId, fileName, targetId)`
- 根据 `api.memory.listAvailableOpeners()` 返回的可用应用动态渲染选项
- 图标映射：跟随上游 `OpenInMenu.tsx` 的 `ICON_BY_ID` 表：

| opener ID | lucide icon |
|-----------|-------------|
| `finder` | `Folder` |
| `cursor` | `FileCode` |
| `vscode` | `FileCode` |
| `zed` | `SquareCode` |
| `xcode` | `Hammer` |
| `ghostty` | `Terminal` |
| `iterm` | `Terminal` |
| `terminal` | `Terminal` |
| `android-studio` | `Smartphone` |
| `antigravity` | `SquareCode` |

未匹配时兜底使用 `FileText`。

### `MemorySection.tsx`（修改）

- 在现有 `Brain` 图标按钮右侧添加 `<OpenInMenu projectId={selectedProjectId} fileName={null} variant="dots" />`
- 与上游 `MemorySection.tsx` 完全一致

### `MemoryView.tsx`（修改）

- 在 Copy 按钮右侧添加 `<OpenInMenu projectId={projectId} fileName={displayedFile} variant="iconMenu" />`
- 当前 Tauri 版已有 Copy 按钮但缺少 OpenInMenu

### `MemoryEntryPreview.tsx`（新建）

- 位置：`src/components/sidebar/memory/MemoryEntryPreview.tsx`
- 复用 `@renderer/components/chat/markdownComponents` + `ReactMarkdown` + `remarkGfm` 渲染 markdown
- `content` 为 `undefined` 时显示 "Loading…"
- 通过 `MemorySection` 下方的展开区域渲染，使用 `memorySlice` 已有的 `expandedEntriesByProjectId`、`fileContents`、`toggleMemoryEntry` 状态

### `shared/types/api.ts` — `MemoryAPI` 扩展

```typescript
export interface OpenTarget {
  id: string;
  name: string;
  iconName: string;   // lucide icon name（与上游 OpenInMenu ICON_BY_ID 对齐）
  available: boolean;
}

export interface MemoryAPI {
  // 已有
  hasMemory: (projectId: string) => Promise<boolean>;
  getIndex: (projectId: string) => Promise<MemoryIndex | null>;
  readFile: (projectId: string, fileName: string) => Promise<MemoryReadFileResult>;
  copyPath: (projectId: string, fileName: string | null) => Promise<MemoryOpenResult>;
  // 新增
  listAvailableOpeners: () => Promise<OpenTarget[]>;
  openIn: (
    projectId: string,
    fileName: string | null,
    openerId: string,
  ) => Promise<MemoryOpenResult>;
}
```

### `api/tauriClient.ts` + `api/httpClient.ts` — 实现

- Tauri：`invoke('list_memory_openers')` / `invoke('open_memory_in', { projectId, fileName, openerId })`
- HTTP：`GET /api/memory/openers` / `POST /api/memory/open-in`（HTTP 模式下 `openIn` 返回失败）

## 错误处理

| 场景 | 处理方式 |
|------|----------|
| 请求的应用未安装 | `open_in` 返回 `Err(AppError::OpenFailed(...))`，前端通过 `MemoryOpenResult.error` 显示 |
| `mdfind` 超时或失败 | 降级到 `/Applications/{name}.app` 路径检查，2s 超时 |
| 文件路径含特殊字符 | Rust 端直接传参给 `Command`，不使用 shell |
| `open -a` 命令失败 | 返回 `Err(AppError::OpenFailed(msg))`，前端通过 `MemoryOpenResult.error` 显示 |
| 无可用 opener | `listAvailableOpeners` 至少返回 Finder（始终可用） |
| HTTP 模式 | `openIn` 返回 `success: false, error: "Not available in HTTP mode"` |

## 安全约束

- `openIn` 命令通过 `OpenTargetId::from_str` 解析 opener_id，未知值返回 `AppError::InvalidInput`
- 路径参数通过 `guards::validate_project_id` 和 `guards::validate_memory_file_name` 校验
- 不使用 `sh -c` 包裹 `open` 命令，所有参数直接传给 `Command`

## 变更文件清单

| 文件 | 变更类型 |
|------|----------|
| `src-tauri/src/services/app_opener.rs` | 新建 |
| `src-tauri/src/services/mod.rs` | 修改（注册 app_opener 模块） |
| `src-tauri/src/error/app_error.rs` | 修改（新增 `OpenFailed` 变体） |
| `src-tauri/src/types/memory.rs` | 修改（新增 `OpenTarget`、`MemoryOpenResult`） |
| `src-tauri/src/services/memory_service_trait.rs` | 修改（新增 2 个 trait 方法） |
| `src-tauri/src/services/memory_service.rs` | 修改（新增 2 个 impl，委托 app_opener） |
| `src-tauri/src/commands/memory.rs` | 修改（新增 2 个命令） |
| `src-tauri/src/http/routes/memory.rs` | 修改（新增 2 个端点 + `OpenInBody`） |
| `src-tauri/src/lib.rs` | 修改（注册新命令） |
| `src/shared/types/api.ts` | 修改（新增 `OpenTarget`，扩展 `MemoryAPI`） |
| `src/api/tauriClient.ts` | 修改（新增 2 个方法） |
| `src/api/httpClient.ts` | 修改（新增 2 个端点） |
| `src/components/sidebar/memory/OpenInMenu.tsx` | 新建 |
| `src/components/sidebar/memory/MemoryEntryPreview.tsx` | 新建 |
| `src/components/sidebar/memory/MemorySection.tsx` | 修改（添加 `⋯` 按钮 + OpenInMenu） |
| `src/components/memory/MemoryView.tsx` | 修改（工具栏添加 OpenInMenu 按钮） |
