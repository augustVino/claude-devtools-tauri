# 迁移进度：v0.4.11 ~ v0.5.0

> 生成日期：2026-05-30
> 设计文档：`docs/superpowers/specs/2026-05-30-memory-viewer-migration-design.md`
> 最后验证日期：2026-05-30（深度代码审查）

## 状态总览

| 功能 | 状态 | 说明 |
|------|------|------|
| Session 更新时间对齐 | ✅ 已迁移 | fingerprint + stale session fix（`updatedAt` 字段 + dateGrouping + 排序 + 时间显示） |
| Git worktree 支持 | ✅ 已迁移 | worktree 会话发现（`git_identity.rs` 向上搜索 `.git` + relative gitdir 解析） |
| **Memory Viewer（核心）** | ✅ 已完成 | 读取 + 索引解析 + 展示（不含 openInLauncher） |
| Memory Viewer（openInLauncher） | 📋 已规划 | 依赖 openInLauncher 子系统 |
| SSH 连接增强 | ⏳ 待迁移 | SshConnectionManager 重写 |
| Hover 复制按钮 | ✅ 已迁移 | LastOutputDisplay 含 CopyButton |
| MermaidViewer 修复 | ✅ 已迁移 | 孤儿 SVG 清理（cleanupOrphans） |

---

## Memory Viewer 迁移明细

### ✅ 已完成

- [x] Rust `MemoryService` trait + impl（`services/memory_service_trait.rs`, `services/memory_service.rs`）
- [x] Rust `commands/memory.rs`（4 个 Tauri 命令：`has_memory`, `get_memory_index`, `read_memory_file`, `copy_memory_path`）
- [x] Rust `http/routes/memory.rs`（3 个 GET 端点 + copy_path POST）
- [x] Rust `types/memory.rs` 新增 memory 相关类型（MemoryIndex, MemoryEntry, MemoryFile, MemoryReadFileResult, MemoryOpenResult）
- [x] Rust MEMORY.md 索引解析（`parse_memory_index` 含 orphan files 检测 + 单元测试）
- [x] 前端 `components/memory/MemoryView.tsx`（主视图，去掉 OpenInMenu）
- [x] 前端 `components/memory/FrontmatterCard.tsx`
- [x] 前端 `components/memory/frontmatter.ts`
- [x] 前端 `components/sidebar/MemorySection.tsx`（去掉 ⋯ 按钮）
- [x] 前端 `store/slices/memorySlice.ts`
- [x] 前端 `shared/utils/memoryIndex.ts`
- [x] 前端 `types/tabs.ts` 新增 `'memory'` 类型
- [x] 前端 `layout/PaneContent.tsx` 新增 memory 分支
- [x] 前端 `layout/Sidebar.tsx` 插入 MemorySection
- [x] 前端 `shared/types/api.ts` 新增简化版 `MemoryAPI`（copyPath 等 4 个方法）
- [x] 前端 `api/` 两种实现（TauriAPIClient `tauriClient.ts:561` + HttpAPIClient `httpClient.ts:745`）
- [x] `lib.rs` 注册 MemoryServiceImpl 为 managed state

### 📋 待迁移（openInLauncher 子系统）

以下项目需在 openInLauncher 迁移完成后补充：

- [ ] Rust `openInLauncher` 模块（跨平台应用检测：Finder/Cursor/VSCode/Zed/Xcode/Ghostty/iTerm/Terminal/Android Studio/Antigravity）
- [ ] Tauri 命令 `list_memory_openers` + `open_memory_in`
- [ ] HTTP 路由 `GET /api/memory/openers` + `POST /api/memory/open-in`
- [ ] 前端 `sidebar/memory/OpenInMenu.tsx`（"Open in..." 下拉菜单组件）
- [ ] 前端 `sidebar/memory/MemoryEntryPreview.tsx`（侧边栏内展开预览）
- [ ] API: `MemoryAPI.listAvailableOpeners()`
- [ ] API: `MemoryAPI.openIn()`
- [ ] `MemorySection.tsx` 补充右侧 `⋯` 按钮
- [ ] `MemoryView.tsx` 工具栏补充 "Open in..." 按钮
- [ ] API: `MemoryAPI.onChanged()` 事件订阅（Memory 文件变更实时刷新）

## 变更日志

| 日期 | 变更 |
|------|------|
| 2026-05-30 | 初始化迁移进度文档 |
| 2026-05-30 | 深度代码审查验证：所有标注 ✅ 的项目均已实际完成，Memory Viewer 核心功能完整 |
