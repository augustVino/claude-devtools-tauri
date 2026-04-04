# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

基于 Tauri v2 的 Claude Code 会话可视化桌面应用。读取 `~/.claude/projects/` 下的 JSONL 会话文件，解析为结构化数据，提供对话浏览、上下文追踪、工具调用分析和错误检测通知等功能。支持本地和 SSH 远程连接。

## Quick Reference

```bash
pnpm install            # 安装依赖
pnpm tauri dev          # 前端 Vite (5173) + Rust 后端同时启动
pnpm build              # 前端 TS 编译 + Vite 打包
pnpm build:macos        # 完整 Tauri 构建 (DMG)
pnpm lint               # ESLint

# Rust 测试
cd src-tauri && cargo test                          # 全部测试 (~552)
cd src-tauri && cargo test -- <module_name>         # 单模块过滤
cd src-tauri && cargo test -p claude-devtools -- <name>  # 按名称过滤

# 前端测试（vitest + happy-dom）
npx vitest run
npx vitest run -- path/to/test.test.tsx
```

## 关键覆盖规则

以下规则覆盖默认行为，修改代码前必须遵守：

- **Tauri API**: `withGlobalTauri: false`，必须从 `@tauri-apps/api` 直接导入，不可使用全局变量
- **路径别名**: `@main/*` → `src/main/*`, `@renderer/*` → `src/*`, `@shared/*` → `src/shared/*`
- **Rust 命令**: snake_case 命名，返回 `Result<T, String>`
- **双传输层**: 新增 API 时需同时实现 `TauriAPIClient` 和 `HttpAPIClient`（通过 `ElectronAPI` 接口统一契约）
- **ReDoS 防护**: `trigger_manager.rs` 的正则必须通过 `regex_validation.rs` 检查
- **唯一数据源**: 文件系统 (`~/.claude/projects/{hash}/*.jsonl`) 为唯一真实来源

## 详细指南

| 文档 | 内容 |
|------|------|
| [Architecture](.claude/architecture.md) | 后端/前端模块结构、数据流、多上下文架构、FsProvider trait、三层类型系统、设计模式 |
| [Development](.claude/development.md) | macOS 特性、Config 深度合并、文件监听器、测试模式、其他编码约定 |
