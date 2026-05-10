# Components

按功能域组织的 UI 组件。

## 目录划分

| 目录 | 职责 |
|------|------|
| `chat/` | 会话消息展示（AI/用户/系统消息组、工具调用、上下文面板） |
| `chat/items/` | 单条消息/工具组件（TextItem、ThinkingItem、LinkedToolItem、SubagentItem 等） |
| `chat/items/linkedTool/` | 工具调用详情渲染（Edit、Read、Write、Skill 各有专用 Viewer） |
| `chat/viewers/` | 内容查看器（CodeBlock、Diff、Markdown、Mermaid） |
| `chat/SessionContextPanel/` | 可见上下文追踪面板（CLAUDE.md、工具输出、用户消息注入展示） |
| `common/` | 跨功能域共享 UI 原语（CopyButton、TokenUsageDisplay、ErrorBoundary 等） |
| `dashboard/` | 概览与列表页 |
| `layout/` | 应用外壳、侧边栏、标题栏 |
| `notifications/` | 通知面板和徽标 |
| `search/` | 搜索 UI 和结果 |
| `settings/` | 设置页（含 NotificationTriggerSettings 子目录） |
| `sidebar/` | 项目/会话导航 |

## 约定

- 跨功能域使用的组件放 `common/`，功能域内组件放在对应目录
- 使用 Tailwind + 主题感知 CSS 变量
- 需要 per-tab 状态的组件通过 `TabUIContext` 访问
- 列表超过 100 项时使用 `@tanstack/react-virtual`
