# openInLauncher 子系统迁移设计

> 日期：2026-05-30
> 上游来源：`claude-devtools/src/main/utils/openInLauncher.ts` + `src/main/ipc/memory.ts`
> 上游前端：`src/renderer/components/sidebar/memory/OpenInMenu.tsx`、`MemoryEntryPreview.tsx`、`MemoryView.tsx`（工具栏部分）

## 概述

将 Electron 的 openInLauncher 子系统迁移到 Tauri，实现跨平台的 Memory 文件/目录"用指定应用打开"功能。本次迁移仅支持 macOS，检测 10 个应用：Finder、Cursor、VS Code、Zed、Xcode、Ghostty、iTerm、Terminal、Android Studio、Antigravity。

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
                                     │ parsing/open_in_launcher.rs│
                                     │ detect_installations()     │
                                     │ open_with()                │
                                     │ (10 个 macOS 检测器)        │
                                     └────────────────────────────┘
```

## 后端设计

### `parsing/open_in_launcher.rs`（新模块）

#### 数据结构

```rust
pub struct OpenTarget {
    pub id: String,        // "finder", "vscode", "cursor" ...
    pub name: String,      // "Finder", "VS Code", "Cursor" ...
    pub icon: Option<String>, // lucide icon name for frontend mapping
    pub available: bool,   // 当前系统是否已安装
}

pub enum OpenTargetId {
    Finder, Cursor, VsCode, Zed, Xcode,
    Ghostty, ITerm, Terminal, AndroidStudio, Antigravity,
}
```

#### 检测策略

| 应用 | 检测方式 | 打开方式 |
|------|----------|----------|
| Finder | 始终可用 | `open <path>`（目录）/ `open -R <path>`（文件选中） |
| VS Code | `mdfind "kMDItemFSName == 'Visual Studio Code.app'"` | `open -a "Visual Studio Code" <path>` |
| Cursor | `mdfind "kMDItemFSName == 'Cursor.app'"` | `open -a "Cursor" <path>` |
| Zed | `mdfind "kMDItemFSName == 'Zed.app'"` | `open -a "Zed" <path>` |
| Xcode | `mdfind "kMDItemFSName == 'Xcode.app'"` | `open -a "Xcode" <path>` |
| Ghostty | `mdfind "kMDItemFSName == 'Ghostty.app'"` | `open -a "Ghostty" <path>` |
| iTerm | `mdfind "kMDItemFSName == 'iTerm.app'"` | `open -a "iTerm" <path>` |
| Terminal | 检查 `/System/Applications/Utilities/Terminal.app` | `open -a "Terminal" <path>` |
| Android Studio | `mdfind "kMDItemFSName == 'Android Studio.app'"` | `open -a "Android Studio" <path>` |
| Antigravity | `mdfind "kMDItemFSName == 'Antigravity.app'"` | `open -a "Antigravity" <path>` |

**Fallback**：`mdfind` 超时 2s 或失败时，降级到 `lsregister` dump 匹配。

#### 实现方式

- 使用 `std::process::Command::new("mdfind")` 执行检测
- 使用 `std::process::Command::new("open")` 执行打开操作
- 不使用 shell（`sh -c`），直接传参数避免注入
- 检测函数纯同步，返回 `Vec<OpenTarget>`

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

在 `MemoryServiceImpl` 中委托 `open_in_launcher.rs` 执行。

### `commands/memory.rs` — 新增 2 命令

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

新增 `OpenInQuery` struct 用于 POST body。

### `types/memory.rs` — 扩展

```rust
pub struct MemoryOpenResult {
    pub success: bool,
    pub path: Option<String>,
    pub error: Option<String>,
}

// OpenTarget 定义在 `types/memory.rs`（需被 commands 和 services 共享）
// `#[serde(rename_all = "camelCase")]` 确保前端收到 camelCase
```

## 前端设计

### `OpenInMenu.tsx`（新建）

- 位置：`src/components/sidebar/memory/OpenInMenu.tsx`
- 使用项目已有的 `Command` 组件（`@renderer/components/common/Command`) 渲染下拉列表
- 两个变体：`folderMenu`（侧边栏用，打开目录）和 `iconMenu`（工具栏用，图标样式）
- 点击时调用 `api.memory.openIn(projectId, fileName, targetId)`
- 根据 `api.memory.listAvailableOpeners()` 返回的可用应用动态渲染选项
- 图标映射：使用 `lucide-react` 内置图标，通过 `OpenTarget.icon` 字段（字符串）映射到对应组件。上游的 `icon` 字段对应关系：`finder` → `FolderOpen`、`vscode` → `Code`、`cursor` → `MousePointer2`、`zed` → `Zap`、`xcode` → `Apple`、`ghostty` → `Terminal`、`iterm` → `Terminal`、`terminal` → `Terminal`、`android-studio` → `Smartphone`、`antigravity` → `Feather`。若 `icon` 为空或未匹配，使用 `FileText` 兜底

### `MemorySection.tsx`（修改）

- 在现有 `Brain` 图标按钮右侧添加 `<OpenInMenu projectId={selectedProjectId} fileName={null} />`
- 与上游完全一致

### `MemoryView.tsx`（修改）

- 在 Copy 按钮右侧添加 `<OpenInMenu projectId={projectId} fileName={displayedFile} variant="iconMenu" />`
- 当前 Tauri 版已有 Copy 按钮但缺少 OpenInMenu

### `MemoryEntryPreview.tsx`（新建）

- 位置：`src/components/sidebar/memory/MemoryEntryPreview.tsx`
- 复用 `@renderer/components/chat/markdownComponents` 渲染 markdown
- 在 `MemorySection` 下方插入展开区域

### `shared/types/api.ts` — `MemoryAPI` 扩展

```typescript
export interface OpenTarget {
  id: string;
  name: string;
  icon?: string;
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

- Tauri：`invoke('list_memory_openers')` / `invoke('open_memory_in', {...})`
- HTTP：`GET /api/memory/openers` / `POST /api/memory/open-in`

## 错误处理

| 场景 | 处理方式 |
|------|----------|
| 请求的应用未安装 | `open_in` 返回错误，前端 toast 提示 |
| `mdfind` 超时或失败 | 降级到 `lsregister` 后备检测，2s 超时 |
| 文件路径含特殊字符 | Rust 端先解码 URL-encoded 路径 |
| `open -a` 命令失败 | 返回 `Err(AppError::OpenFailed(msg))` |
| 无可用 opener | `listAvailableOpeners` 至少返回 Finder（始终可用） |

## 安全约束

- `openIn` 命令仅接受 `openerId` 白名单中的值
- 路径参数通过 `guards::validate_project_id` 和 `guards::validate_memory_file_name` 校验
- 不使用 `sh -c` 包裹 `open` 命令

## 变更文件清单

| 文件 | 变更类型 |
|------|----------|
| `src-tauri/src/parsing/open_in_launcher.rs` | 新建 |
| `src-tauri/src/parsing/mod.rs` | 修改（注册新模块） |
| `src-tauri/src/types/memory.rs` | 修改（新增 OpenTarget） |
| `src-tauri/src/services/memory_service_trait.rs` | 修改（新增 2 个 trait 方法） |
| `src-tauri/src/services/memory_service.rs` | 修改（新增 2 个 impl） |
| `src-tauri/src/commands/memory.rs` | 修改（新增 2 个命令） |
| `src-tauri/src/http/routes/memory.rs` | 修改（新增 2 个端点） |
| `src-tauri/src/lib.rs` | 修改（注册新命令） |
| `src/shared/types/api.ts` | 修改（扩展 MemoryAPI） |
| `src/api/tauriClient.ts` | 修改（新增 2 个方法） |
| `src/api/httpClient.ts` | 修改（新增 2 个端点） |
| `src/components/sidebar/memory/OpenInMenu.tsx` | 新建 |
| `src/components/sidebar/memory/MemoryEntryPreview.tsx` | 新建 |
| `src/components/sidebar/memory/MemorySection.tsx` | 修改（添加 `⋯` 按钮） |
| `src/components/memory/MemoryView.tsx` | 修改（工具栏添加按钮） |
