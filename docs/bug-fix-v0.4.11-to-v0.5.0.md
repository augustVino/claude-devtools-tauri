# Bug 修复计划：v0.4.11 ~ v0.5.0 上游同步

> 基于 `upstream-v0.4.11-to-v0.5.0-changes.md` 中的 bug fix 分析
> 最后验证日期：2026-05-30（深度代码审查）

## 修复批次

### Batch 1：Session 数据正确性（A + B）

| # | Bug | 模块 | 状态 | 验证说明 |
|---|-----|------|------|----------|
| A-1 | `Session.updatedAt` 字段缺失（根因） | Rust types + discovery + 前端 types | ✅ 已完成 | `domain.rs:114-115` — `updated_at: Option<u64>` 已定义，`skip_serializing_if = "Option::is_none"`；`project_scanner.rs` 和 `session_service.rs` 均设置 `updated_at: Some(mtime_ms)` |
| A-2 | `dateGrouping` 使用 `createdAt` 而非 `updatedAt` | 前端 utils | ✅ 已完成 | `dateGrouping.ts:31` — 使用 `Math.max(session.updatedAt ?? session.createdAt, session.createdAt)`，优先取 `updatedAt` |
| A-3 | `SessionItem` 时间显示 + 侧边栏排序基于 `createdAt` | 前端 components + store | ✅ 已完成 | `SessionItem.tsx:290` — 时间显示使用 `Math.max(session.updatedAt ?? session.createdAt, session.createdAt)`；`sessionSlice.ts:122-124` — 排序使用 `(b.updatedAt ?? b.createdAt) - (a.updatedAt ?? a.createdAt)` |
| B-1 | DataCache 无 fingerprint 字段 | Rust infrastructure | ✅ 已完成 | `data_cache.rs` — `SessionCacheEntry` 有 `fingerprint: Option<String>` 字段；`get`/`set` 方法均接受 `fingerprint: Option<&str>` 参数；包含 3 个测试用例覆盖 mismatch/none/overwrite 场景 |
| B-2 | `get_session_detail` 不支持 fingerprint 短路 | Rust service + command + 前端 API + store | ✅ 已完成 | `session_service_trait.rs` — trait 签名含 `known_fingerprint: Option<&str>`；`commands/sessions.rs` — 命令接收 `known_fingerprint`；`http/routes/sessions.rs` — query 参数支持；前端 `sessionDetailSlice.ts` — `sessionFileFingerprint` Map 缓存 + 短路逻辑完整 |

### Batch 2：Git Worktree + UI（C + D）

| # | Bug | 模块 | 状态 | 验证说明 |
|---|-----|------|------|----------|
| C-1 | `git_identity.rs` 5 个方法不向上搜索 `.git` | Rust parsing | ✅ 已完成 | `git_identity.rs` — `resolve_identity` 先检查 `.git` 存在性，is_file（worktree）时解析 gitdir 找 main repo，is_dir（main repo）时直接使用；fallback `resolve_identity_from_path` 覆盖路径不存在场景；7 种 worktree source 模式识别完整 |
| C-2 | `get_branch` / `get_git_worktree_name` 不解析 relative gitdir | Rust parsing | ✅ 已完成 | `git_identity.rs:339-369` — `get_branch` 对 worktree 先解析 `.git` 文件中的 gitdir，再用 `PathBuf::from(git_dir).join("HEAD")` 读取；`git_identity.rs:506-525` — `get_git_worktree_name` 同样解析 relative gitdir |
| D-1 | MermaidViewer orphan SVG 残留 | 前端 components | ✅ 已完成 | `MermaidViewer.tsx:67-70` — `cleanupOrphans` 移除 `d{id}` 和 `{id}` 残留节点；`MermaidViewer.tsx:131` — render 成功后 `finally` 中调用；`MermaidViewer.tsx:137` — effect cleanup 中调用；commit `ec4561c` 已合入 |
| D-2 | LastOutputDisplay hover copy 缺失 | 前端 components | ✅ 已完成 | `LastOutputDisplay.tsx:91` — 文本输出使用 `<CopyButton text={textContent} />`；`LastOutputDisplay.tsx:235` — plan_exit 场景也包含 CopyButton；跟随上游实现 |

## 状态说明

- ⬜ 待修复
- 🔧 修复中
- ✅ 已完成
- ❌ 跳过

## 变更日志

| 日期 | 变更 |
|------|------|
| 2026-05-30 | 初始化修复计划 |
| 2026-05-30 | 验证完成：Batch 1 (A-1~A-3, B-1~B-2)、Batch 2 (C-1~C-2) 均已完成，仅剩 D-1、D-2 |
| 2026-05-30 | D-1 深度排查确认已实现（cleanupOrphans + finally/cleanup 全覆盖），D-2 添加文本输出 CopyButton（跟随上游） |
| 2026-05-30 | 全部 10 个 bug fix 项目（A-1~A-3, B-1~B-2, C-1~C-2, D-1~D-2）深度验证完成，确认全部实现 |
