# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 项目概述

基于 Tauri v2 的 Claude Code 会话可视化桌面应用。读取 `~/.claude/projects/` 下的 JSONL 会话文件，解析为结构化数据，提供对话浏览、上下文追踪、工具调用分析和错误检测通知等功能。支持本地文件系统和 SSH 远程连接。从 [claude-devtools](https://github.com/matt1398/claude-devtools) (Electron) 移植而来。

## 常用命令

```bash
pnpm install            # 安装依赖
pnpm tauri dev          # 前端 Vite (5173) + Rust 后端同时启动
pnpm build              # 仅前端 TypeScript 编译 + Vite 打包
pnpm build:macos        # 完整 Tauri 构建 (DMG)
pnpm lint               # ESLint
```

## 技术栈

- **后端**: Rust + Tauri v2.10, tokio, serde, axum 0.8 (HTTP), russh 0.46 (SSH)
- **前端**: React 19, TypeScript 5.9, Vite 8, Tailwind CSS 3, Zustand 5

## 详细文档

- [Architecture](.claude/architecture.md) — 后端/前端模块结构、数据流、多上下文架构、设计模式
- [Development](.claude/development.md) — 开发注意事项、测试模式、编码约定、macOS 特性
