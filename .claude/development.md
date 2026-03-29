# Development Notes

## Critical Overrides

- **`withGlobalTauri: false`**: Tauri API 不暴露为全局变量，必须从 `@tauri-apps/api` 直接导入
- **路径别名**: `@main/*` → `src/main/*`, `@renderer/*` → `src/*`, `@shared/*` → `src/shared/*`

## macOS Specifics

- 私有 API 已启用（Dock 隐藏），使用 `cocoa` + `objc` crate
- 窗口使用 Overlay 标题栏样式，traffic lights 位于 (16, 18)
- 启动时隐藏 (`visible: false`)，初始化完成后显示
- Bundle 目标仅 DMG，最低 macOS 10.15

## Coding Conventions

- Tauri 命令名称使用 snake_case，前端调用时通过 `invoke` 映射
- Rust 命令返回 `Result<T, String>`，错误为序列化字符串
- `trigger_manager.rs` 中的正则需通过 `regex_validation.rs` 的 ReDoS 防护检查

## Testing

### Rust Tests

使用 `tempfile` 创建临时文件，异步测试使用 `#[tokio::test]`。

```bash
cd src-tauri && cargo test                          # 全部测试
cd src-tauri && cargo test -- config_manager        # 单模块测试
cd src-tauri && cargo test -p claude-devtools-tauri -- chunk_builder  # 按名称过滤
```

测试覆盖的主要模块：`chunk_builder`、`ssh_connection_manager`、`ssh_config_parser`、`context_manager`、`fs_provider`、`sse`、`server`、`error_trigger_tester`。

### Frontend Tests

基础设施已就绪 (vitest + happy-dom + @testing-library/react)，尚未编写测试文件。

```bash
npx vitest run
npx vitest run -- path/to/test.test.ts
```

## File Watchers

三个并发文件监听器：
1. 主监听器 (JSONL/JSON)
2. 错误检测管道 (仅非子 Agent JSONL)
3. Todo 监听器

所有监听器使用 notify-debouncer-mini，100ms debounce。
