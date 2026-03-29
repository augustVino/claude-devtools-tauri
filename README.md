# claude-devtools-tauri

[![Download](https://img.shields.io/github/v/release/augustVino/claude-devtools-tauri?label=Download&style=for-the-badge)](https://github.com/augustVino/claude-devtools-tauri/releases/latest)

基于 [Tauri v2](https://tauri.app/) 的 Claude Code 会话可视化桌面应用 — 浏览对话记录、追踪上下文窗口使用情况、分析工具调用。

## Why

[claude-devtools](https://github.com/matt1398/claude-devtools) 是一个优秀的 Claude Code 会话分析工具，但它基于 Electron 构建。Electron 打包后体积较大（通常 120MB+），且运行时内存占用高。本项目的目标是在保留原项目前端设计的基础上，用 Rust 重写后端，通过 Tauri v2 实现更小的安装包和更低的资源占用。

## 功能

- 可视化 Claude Code 会话时间线
- 追踪上下文窗口在 6 个分类中的使用情况
- 分析工具调用与子 Agent / Team 编排
- 跨会话搜索（全文检索）
- 实时文件监听与增量更新
- 错误检测与通知触发（原生 OS 通知）
- macOS 系统托盘 + Dock 隐藏
- SSH 远程连接 — 通过 SSH 浏览远程机器上的 Claude 会话
- 多上下文切换 — 本地 / SSH 远程上下文之间切换
- 内置 HTTP 服务 — 浏览器端访问（REST API + SSE 实时事件流）

## 开发

### 环境要求

- [Rust](https://rustup.rs/) 1.77.2+
- [Node.js](https://nodejs.org/) 18+
- [pnpm](https://pnpm.io/) 8+

### 安装与运行

```bash
pnpm install
pnpm tauri dev
```

### 构建

```bash
pnpm tauri build          # macOS: DMG / Windows: NSIS / Linux: deb+rpm
```

### 测试

```bash
cd src-tauri && cargo test               # Rust 单元测试 (552)
cd src-tauri && cargo test -- session_   # 按名称过滤
```

## 技术栈

- **后端：** Rust + Tauri v2.10, tokio (异步运行时), serde, axum 0.8 (HTTP), russh 0.46 (SSH)
- **前端：** React 19, TypeScript 5.9, Vite 8, Tailwind CSS 3, Zustand 5
- **核心 crates：** tokio, serde, notify (文件监听), moka (LRU 缓存), russh (SSH), axum (HTTP)

## 项目规模

| 指标 | 数值 |
|------|------|
| Rust 后端 | 92 文件, ~27,900 LOC, 552 单元测试 |
| 前端 | ~43,800 LOC TypeScript/TSX, 14 Zustand store slices |
| Tauri 命令 | 65 个 `#[tauri::command]` |
| HTTP 端点 | 50+ REST + SSE |
| 事件通道 | 8 个 (file-change, todo-change, notification, error, ssh, context 等) |

## 许可证

[MIT](./LICENSE) — 归属详情见 [NOTICE](./NOTICE)。

## 致谢

本项目衍生自 [claude-devtools](https://github.com/matt1398/claude-devtools)，由 matt1398 及贡献者开发，采用 MIT 许可证。
