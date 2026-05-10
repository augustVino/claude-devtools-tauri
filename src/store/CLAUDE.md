# Store (Zustand)

基于 slices 模式的状态管理。

## 文件结构

- `index.ts` — Store 创建，合并所有 slice + 事件监听初始化
- `types.ts` — AppState 类型定义
- `slices/` — 各领域 slice
- `utils/` — 工具函数（paneHelpers、pathResolution、batchAsync、stateResetHelpers）

## Slices（15 个）

| Slice | 职责 |
|-------|------|
| `projectSlice` | 项目列表，selectedProjectId |
| `repositorySlice` | 仓库分组，worktree |
| `sessionSlice` | 会话列表，分页，selectedSessionId |
| `sessionDetailSlice` | 会话详情，chunks，metrics |
| `subagentSlice` | 子 Agent 数据 |
| `conversationSlice` | 消息，对话元数据 |
| `tabSlice` | Tab 列表，activeTabId，排序 |
| `tabUISlice` | Per-tab UI 状态（展开、滚动位置） |
| `paneSlice` | 窗格布局，分屏 |
| `uiSlice` | UI 标志（sidebar 可见性等） |
| `notificationSlice` | 通知，unreadCount |
| `configSlice` | 应用配置，触发器 |
| `connectionSlice` | 连接模式（local/ssh），连接状态 |
| `contextSlice` | 上下文切换（local/ssh），activeContextId |
| `updateSlice` | 应用更新检查 |

## 关键模式: Per-Tab UI 隔离

`tabUISlice` 按 tabId 维护独立的 UI 状态（`expandedAIGroupIds` 等），确保 tab A 的展开状态不影响 tab B。

## 初始化

`initializeNotificationListeners()` 在 App.tsx 的 useEffect 中调用一次：
- 订阅 file-change / todo-change 事件
- 自适应 debounce 刷新（短会话 150ms，长会话最高 60s）
- 使用 `refreshSessionInPlace` 防止闪烁

## 新增 Slice

1. 创建 `slices/{domain}Slice.ts`，导出 `create{Domain}Slice`
2. 在 `index.ts` 中合并
3. 更新 `types.ts` 的 `AppState`
