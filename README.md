# claude-devtools-tauri

基于 [Tauri v2](https://tauri.app/) 的 Claude Code 会话可视化桌面应用 — 浏览对话记录、追踪上下文窗口使用情况、分析工具调用。

## Why

[claude-devtools](https://github.com/matt1398/claude-devtools) 是一个优秀的 Claude Code 会话分析工具，但它基于 Electron 构建。Electron 打包后体积较大（通常 120MB+），且运行时内存占用高。本项目的目标是在保留原项目前端设计的基础上，用 Rust 重写后端，通过 Tauri v2 实现更小的安装包和更低的资源占用。

## 功能

- 可视化 Claude Code 会话时间线
- 追踪上下文窗口在 6 个分类中的使用情况
- 分析工具调用与子 Agent 编排
- 跨会话搜索
- 实时文件监听与增量更新
- 错误检测与通知触发

## V2 待实现

以下功能计划在后续版本中完成：

- **SSH 远程连接** — 通过 SSH 访问远程机器上的 Claude 会话（8 个命令）
- **上下文切换** — 多 Claude 实例间的上下文切换（3 个命令）
- **内置 HTTP 服务** — 提供浏览器端访问的后端服务（3 个命令）

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
pnpm tauri build
```

## 技术栈

- **后端：** Rust + Tauri v2
- **前端：** React 18, TypeScript, Vite, Tailwind CSS, Zustand
- **核心 crates：** tokio, serde, notify, moka

## 许可证

[MIT](./LICENSE) — 归属详情见 [NOTICE](./NOTICE)。

## 致谢

本项目衍生自 [claude-devtools](https://github.com/matt1398/claude-devtools)，由 matt1398 及贡献者开发，采用 MIT 许可证。
