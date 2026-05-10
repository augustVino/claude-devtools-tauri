# Renderer

React 前端，通过 `src/api/` 的 Proxy 层与后端通信，不感知 Tauri IPC 还是 HTTP。

## 关键目录

| 目录 | 职责 |
|------|------|
| `components/` | UI 组件，按功能域组织（chat、sidebar、search、settings 等） |
| `store/` | Zustand 状态管理，slices 模式 |
| `hooks/` | 自定义 hooks（useAutoScrollBottom、useKeyboardShortcuts、useTabNavigationController、useVisibleAIGroup 等） |
| `utils/` | 渲染层工具函数（contextTracker、claudeMdTracker、toolLinkingEngine、displayItemBuilder） |
| `types/` | 渲染层类型定义（data、groups、contextInjection、panes、tabs） |
| `shared/` | 跨进程共享类型和纯工具函数 |
| `constants/` | CSS 变量、布局常量、团队颜色 |
| `contexts/` | TabUIContext — per-tab UI 状态隔离 |

## 模式与约定

- **API 调用**: 统一通过 `import { api } from "@renderer/api"` 使用，不要直接调用 `@tauri-apps/api`
- **状态管理**: Zustand slices，每个 slice 自含 data/selectedId/loading/error；新增 slice 见 `store/CLAUDE.md`
- **虚拟滚动**: 列表超过 100 项时使用 `@tanstack/react-virtual`（会话列表、消息列表）
- **Per-Tab 隔离**: 通过 `TabUIContext` + `tabUISlice` 实现多 tab 间 UI 状态独立
- **路径别名**: `@renderer/*` → `src/*`, `@shared/*` → `src/shared/*`
