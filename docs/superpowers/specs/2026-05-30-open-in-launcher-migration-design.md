# openInLauncher 子系统迁移设计

> 日期：2026-05-30
> 最后修订：2026-05-30（第二次深度代码审查修复）
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
                                     ┌─────────────▼──────────────────┐
                                     │ commands/memory.rs 直接调用：   │
                                     │ - crate::utils::app_opener::   │
                                     │     detect_installations()     │
                                     │ - MemoryService 的 open_in：   │
                                     │   解析路径 → open_with()       │
                                     └────────┬───────────────────────┘
                                              │
                                ┌─────────────▼──────────────┐
                                │ utils/app_opener.rs        │
                                │ (新模块，纯工具函数)        │
                                │ detect_installations()     │
                                │ open_with()                │
                                │ (10 个 macOS 检测器)        │
                                └────────────────────────────┘
```

**关键设计决策**：
- `app_opener.rs` 放 `utils/` 而非 `services/`：该模块是纯系统工具函数（进程调用 + 路径检测），不涉及业务逻辑或状态管理。`services/` 中所有模块均遵循 trait+impl 模式，放进去会打破既有结构。
- `list_memory_openers` 命令直接调用 `utils::app_opener::detect_installations()`，**不通过 MemoryService trait**。该函数零参数，与 memory 业务无关。
- `open_memory_in` 命令通过 `MemoryService` 获取路径（`get_dir_path`/`get_file_path`），然后委托 `utils::app_opener::open_with()` 执行打开操作。

## 后端设计

### `utils/app_opener.rs`（新模块）

#### 数据结构

```rust
/// 可用来打开 Memory 文件/目录的外部应用目标。
/// 序列化时用 camelCase，前端收到 `iconName` 和 `label`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenTarget {
    pub id: String,        // "finder", "vscode", "cursor" ...
    pub label: String,     // "Finder", "VS Code", "Cursor" ...（前端显示用）
    pub icon_name: String, // lucide icon name
    pub available: bool,   // 当前系统是否已安装
    pub shortcut_key: Option<String>, // 快捷键提示，如 "⌘O", "1"
}

/// opener_id 白名单枚举。命令层解析传入的 opener_id 字符串，
/// 未知值返回 AppError::InvalidInput。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenTargetId {
    Finder, Cursor, VsCode, Zed, Xcode,
    Ghostty, ITerm, Terminal, AndroidStudio, Antigravity,
}

impl std::str::FromStr for OpenTargetId {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "finder" => Ok(Self::Finder),
            "cursor" => Ok(Self::Cursor),
            "vscode" => Ok(Self::VsCode),
            "zed" => Ok(Self::Zed),
            "xcode" => Ok(Self::Xcode),
            "ghostty" => Ok(Self::Ghostty),
            "iterm" => Ok(Self::ITerm),
            "terminal" => Ok(Self::Terminal),
            "android-studio" => Ok(Self::AndroidStudio),
            "antigravity" => Ok(Self::Antigravity),
            _ => Err(format!("Unknown opener: {s}")),
        }
    }
}
```

#### 检测策略

| 应用 | label | 检测方式 | 打开方式 | 快捷键 |
|------|-------|----------|----------|--------|
| Finder | Finder | 始终可用 | `open <path>`（目录）/ `open -R <path>`（文件选中） | — |
| VS Code | VS Code | `mdfind "kMDItemFSName == 'Visual Studio Code.app'"`，fallback 检查 `/Applications/Visual Studio Code.app` | `open -a "Visual Studio Code" <path>` | ⌘O |
| Cursor | Cursor | `mdfind "kMDItemFSName == 'Cursor.app'"`，fallback 检查 `/Applications/Cursor.app` | `open -a "Cursor" <path>` | 1 |
| Zed | Zed | `mdfind "kMDItemFSName == 'Zed.app'"`，fallback 检查 `/Applications/Zed.app` | `open -a "Zed" <path>` | 2 |
| Xcode | Xcode | `mdfind "kMDItemFSName == 'Xcode.app'"`，fallback 检查 `/Applications/Xcode.app` | `open -a "Xcode" <path>` | 3 |
| Ghostty | Ghostty | `mdfind "kMDItemFSName == 'Ghostty.app'"`，fallback 检查 `/Applications/Ghostty.app` | `open -a "Ghostty" <path>` | 4 |
| iTerm | iTerm | `mdfind "kMDItemFSName == 'iTerm.app'"`，fallback 检查 `/Applications/iTerm.app` | `open -a "iTerm" <path>` | 5 |
| Terminal | Terminal | 检查 `/System/Applications/Utilities/Terminal.app` | `open -a "Terminal" <path>` | 6 |
| Android Studio | Android Studio | `mdfind "kMDItemFSName == 'Android Studio.app'"`，fallback 检查 `/Applications/Android Studio.app` | `open -a "Android Studio" <path>` | 7 |
| Antigravity | Antigravity | `mdfind "kMDItemFSName == 'Antigravity.app'"`，fallback 检查 `/Applications/Antigravity.app` | `open -a "Antigravity" <path>` | 8 |

**Fallback 策略**：`mdfind` 失败或未找到时，使用 `std::fs::metadata` 检查 `/Applications/{name}.app`。不使用 `lsregister`。

#### 并行检测

所有 10 个应用的检测**并发执行**（对齐上游 `Promise.all`），使用 `futures::future::join_all()`。每个检测独立 2s 超时：

```rust
let detections: Vec<_> = APP_SPECS.iter().map(|spec| async move {
    match tokio::time::timeout(
        Duration::from_secs(2),
        detect_single_app(spec),
    ).await {
        Ok(result) => result,
        Err(_) => AppDetectorResult { available: false }, // timeout = not available
    }
}).collect();
let results = futures::future::join_all(detections).await;
```

#### 异步与超时

- 使用 `tokio::process::Command` 执行 `mdfind` 和 `open`，不阻塞 async runtime
- 每个 mdfind 调用独立 2s 超时（`tokio::time::timeout`）
- `open -a` 操作不设超时（由系统调度）

#### 安全

- 不使用 `sh -c` 包裹命令，所有参数直接传给 `Command::new()`
- `OpenTargetId::from_str` 作为白名单：无法解析的 opener_id 返回 `Err`

#### 非 macOS 平台

- `detect_installations()` 在非 macOS 平台返回空 Vec
- `open_with()` 在非 macOS 平台返回 `Err(AppError::OpenFailed("Not supported on this platform"))`
- 整个模块编译不依赖 macOS 特有 API（`mdfind` 和 `open -a` 在运行时调用，编译时无特殊依赖）

### `services/memory_service.rs` — trait 扩展

在 `MemoryService` trait 中**仅新增 1 个方法**：

```rust
async fn open_in(
    &self,
    opener_id: &str,
    project_id: &str,
    file_name: Option<&str>,
) -> Result<(), AppError>;
```

`list_available_openers` **不加入 trait**——由 `commands/memory.rs` 直接调用 `crate::utils::app_opener::detect_installations()`。

在 `MemoryServiceImpl` 中 `open_in` 的实现逻辑：
1. 解析 `opener_id` → `OpenTargetId`（白名单校验，未知值返回 `AppError::InvalidInput`）
2. 根据 `file_name` 调用 `get_file_path` 或 `get_dir_path` 获取绝对路径
3. 委托 `crate::utils::app_opener::open_with(opener_id, &path, is_directory)`

### `error/app_error.rs` — 新增变体

```rust
OpenFailed(String),
```

用于 `open -a` 命令失败、应用未安装、非 macOS 平台等场景。与 `FileOp` 区分：`FileOp` 用于文件读写操作失败，`OpenFailed` 用于外部应用打开失败。

### `commands/memory.rs` — 新增 2 命令

在现有 4 个命令后追加：

```rust
#[command]
pub async fn list_memory_openers() -> Result<Vec<OpenTarget>, String> {
    // 直接调用 utils 层，不经 MemoryService
    Ok(crate::utils::app_opener::detect_installations().await)
}

#[command]
pub async fn open_memory_in(
    service: State<'_, Arc<dyn MemoryService>>,
    project_id: String,
    file_name: Option<String>,
    opener_id: String,
) -> Result<MemoryOpenResult, String> {
    let safe_id = guards::validate_project_id(&project_id)
        .map_err(|e| { log::error!("Invalid projectId: {e}"); e })?;
    let safe_name = file_name.as_ref().filter(|n| !n.trim().is_empty())
        .map(|n| guards::validate_memory_file_name(n))
        .transpose()
        .map_err(|e| { log::error!("Invalid fileName: {e}"); e })?;
    match service.open_in(&opener_id, &safe_id, safe_name.as_deref()).await {
        Ok(()) => Ok(MemoryOpenResult { success: true, path: None, error: None }),
        Err(e) => Ok(MemoryOpenResult { success: false, path: None, error: Some(e.to_string()) }),
    }
}
```

注意：`file_name` 为 `None` 或空字符串时跳过 `validate_memory_file_name`（打开目录场景），与现有 `copy_memory_path` 命令一致。

### `http/routes/memory.rs` — 新增 2 端点

- `GET /api/memory/openers` → `Json<Vec<OpenTarget>>`
- `POST /api/memory/open-in` → `Json<MemoryOpenResult>`

新增 `OpenInBody` struct：

```rust
#[derive(Deserialize)]
pub struct OpenInBody {
    pub project_id: String,
    #[serde(default)]
    pub file_name: Option<String>,
    pub opener_id: String,
}
```

**HTTP 模式限制**：`open_memory_in` HTTP handler 直接返回：
```rust
Ok(Json(MemoryOpenResult {
    success: false,
    path: None,
    error: Some("Open operations are not available in HTTP mode".into()),
}))
```
不使用 `require_project_id`，因为该操作在 HTTP 模式下完全不可用。与上游 `httpClient.ts` 中 `openIn` 返回 `{ success: false, error: 'Open-in is unsupported in standalone mode' }` 一致。

`list_memory_openers` HTTP handler 正常调用 `detect_installations()`——HTTP 模式下仍然需要展示 opener 列表（至少返回 Finder 等检测到的应用）。

### `http/routes/mod.rs` — 注册路由

在 `build_routes()` 中现有 memory 路由之后追加：

```rust
.route("/api/memory/openers", get(memory::list_memory_openers))
.route("/api/memory/open-in", post(memory::open_memory_in))
```

### `types/memory.rs` — 扩展

仅新增 `OpenTarget`。`MemoryOpenResult` **已存在**（line 47-55），无需修改。

```rust
/// 可打开的外部应用目标（与 utils/app_opener.rs 中的 OpenTarget 一致）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenTarget {
    pub id: String,
    pub label: String,
    pub icon_name: String,
    pub available: bool,
    pub shortcut_key: Option<String>,
}
```

### `utils/mod.rs` — 注册新模块

```rust
pub mod app_opener;
```

### `lib.rs` — 注册新命令

在 `invoke_handler!` 中现有 memory 命令（`commands::memory::copy_memory_path`）之后追加：

```rust
commands::memory::list_memory_openers,
commands::memory::open_memory_in,
```

### 单元测试

- `OpenTargetId::from_str`：10 个已知变体 + 1 个未知值拒绝
- `OpenTarget` 序列化：验证 camelCase 输出（`iconName`、`label` 字段名正确）
- `detect_installations`：macOS 上实际运行测试，非 macOS 上验证返回空 Vec
- `open_with`：验证 `Command` 参数构建正确性（不实际执行打开）

## 前端设计

### `OpenInMenu.tsx`（新建）

- 位置：`src/components/sidebar/memory/OpenInMenu.tsx`
- **不使用 `Command` 组件**（该项目不存在）。使用纯 React 状态 + DOM 事件的 custom dropdown：
  - `absolute` 定位（包裹在 `relative inline-block` 容器中）
  - `pointerdown` 外部点击关闭（`document.addEventListener`）
  - `Escape` 键关闭
  - Tailwind + CSS 变量样式（`var(--color-surface-overlay)`、`var(--color-border-emphasis)`）
- 两个变体：`'dots'`（侧边栏 `⋯` 按钮）和 `'iconMenu'`（工具栏图标样式）
- 点击 opener 项时调用 `api.memory.openIn(projectId, fileName, targetId)`
- 根据 `api.memory.listAvailableOpeners()` 返回的可用应用动态渲染选项
- **Copy Path 菜单项**：在 opener 列表底部添加分割线 + "Copy Path" 项（图标 `Clipboard`，快捷键 `⌘⇧C`），调用 `api.memory.copyPath(projectId, fileName)` 后关闭菜单。与上游 `OpenInMenu.tsx` 一致。
- **加载中状态**：`listAvailableOpeners()` 调用期间显示 "Detecting apps..."
- 图标映射：跟随上游 `OpenInMenu.tsx` 的 `ICON_BY_ID` 表：

| opener ID | lucide icon | 快捷键 |
|-----------|-------------|--------|
| `finder` | `Folder` | — |
| `cursor` | `FileCode` | 1 |
| `vscode` | `FileCode` | ⌘O |
| `zed` | `SquareCode` | 2 |
| `xcode` | `Hammer` | 3 |
| `ghostty` | `Terminal`（导入时 `Terminal as TerminalIcon`） | 4 |
| `iterm` | `Terminal`（同上） | 5 |
| `terminal` | `Terminal`（同上） | 6 |
| `android-studio` | `Smartphone` | 7 |
| `antigravity` | `SquareCode` | 8 |
| `copy-path` | `Clipboard` | ⌘⇧C |

注意：`Terminal` 导入时使用 `Terminal as TerminalIcon` 避免与 HTML 元素类型名冲突。未匹配时兜底使用 `FileText`。

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
- 简单纯展示组件，不管理展开状态（状态由父组件通过 props 传入）

### `shared/types/api.ts` — `MemoryAPI` 扩展

```typescript
export interface OpenTarget {
  id: string;
  label: string;       // 与上游 OpenInMenu 的 target.label 对齐
  iconName: string;
  available: boolean;
  shortcutKey?: string; // 快捷键提示，如 "⌘O", "1"
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
- HTTP：`GET /api/memory/openers` / `POST /api/memory/open-in`
- HTTP `listAvailableOpeners`：正常调用后端端点（返回 opener 列表）
- HTTP `openIn`：后端返回 `success: false`，前端正常处理 `MemoryOpenResult`

### 错误反馈

当前项目无 toast 库。`openIn` 失败时，使用 `MemoryOpenResult.error` 字段在前端显示 inline 错误提示（跟随现有 Copy 按钮 "Copied" 状态模式——在按钮区域短暂显示错误文本）。

## 错误处理

| 场景 | 处理方式 |
|------|----------|
| 请求的应用未安装 | `open_in` 返回 `Err(AppError::OpenFailed(...))`，前端 `MemoryOpenResult.error` 显示 |
| `mdfind` 超时或失败 | 降级到 `/Applications/{name}.app` 路径检查，2s 超时 |
| 文件路径含特殊字符 | Rust 端直接传参给 `Command`，不使用 shell |
| `open -a` 命令失败 | 返回 `Err(AppError::OpenFailed(msg))` |
| 无可用 opener | `listAvailableOpeners` 至少返回 Finder（始终可用） |
| HTTP 模式 `openIn` | 返回 `success: false, error: "Not available in HTTP mode"` |
| 非 macOS 平台 | `detect_installations` 返回空 Vec，`open_with` 返回 `Err(OpenFailed)` |

## 安全约束

- `openIn` 命令通过 `OpenTargetId::from_str` 解析 opener_id，未知值返回 `AppError::InvalidInput`
- 路径参数通过 `guards::validate_project_id` 和条件性 `guards::validate_memory_file_name` 校验
- 不使用 `sh -c` 包裹 `open` 命令，所有参数直接传给 `Command`

## 变更文件清单

| 文件 | 变更类型 |
|------|----------|
| `src-tauri/src/utils/app_opener.rs` | 新建 |
| `src-tauri/src/utils/mod.rs` | 修改（注册 app_opener 模块） |
| `src-tauri/src/error/app_error.rs` | 修改（新增 `OpenFailed` 变体） |
| `src-tauri/src/types/memory.rs` | 修改（新增 `OpenTarget`；`MemoryOpenResult` 已存在） |
| `src-tauri/src/services/memory_service_trait.rs` | 修改（新增 `open_in` 方法） |
| `src-tauri/src/services/memory_service.rs` | 修改（新增 `open_in` impl，委托 app_opener） |
| `src-tauri/src/commands/memory.rs` | 修改（新增 2 个命令） |
| `src-tauri/src/http/routes/memory.rs` | 修改（新增 2 个端点 + `OpenInBody`） |
| `src-tauri/src/http/routes/mod.rs` | 修改（注册 2 个新路由） |
| `src-tauri/src/lib.rs` | 修改（注册新命令） |
| `src/shared/types/api.ts` | 修改（新增 `OpenTarget`，扩展 `MemoryAPI`） |
| `src/api/tauriClient.ts` | 修改（新增 2 个方法） |
| `src/api/httpClient.ts` | 修改（新增 2 个端点） |
| `src/components/sidebar/memory/OpenInMenu.tsx` | 新建 |
| `src/components/sidebar/memory/MemoryEntryPreview.tsx` | 新建 |
| `src/components/sidebar/memory/MemorySection.tsx` | 修改（添加 `⋯` 按钮 + OpenInMenu） |
| `src/components/memory/MemoryView.tsx` | 修改（工具栏添加 OpenInMenu 按钮） |
