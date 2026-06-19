# ROADMAP — 已知债务与推迟项

> 本文件入库，记录所有"推迟到未来阶段"的技术债务。
> 每项含：描述、代码位置（file:line 或符号锚点）、推迟原因、恢复触发条件。
> 完成的项移到 §已完成（保留历史，不删除）。

## 进行中（phase 3e 第一批遗留）

### clippy warnings 待批量修复
- **位置**：全仓 `src-tauri/src/**`（lib target 196 个 + lib test target 221 个）
- **推迟原因**：3e 第一批聚焦 CI 保护伞，不修业务 warning
- **恢复触发**：phase 3e Item 4（pre-commit hooks）前置任务
- **完成后**：CI clippy 步骤加 `-- -D warnings`（`.github/workflows/ci.yml` 内 TODO(phase-3e-item4)）

### 全量 cargo fmt 待独立 PR（v4 新增，codex P0-1）
- **位置**：全仓 `src-tauri/src/**`（实测 146 文件 / 941 diff 位置 / 13,846 行，含 build.rs）
- **当前状态**：v4 仅对 Task -1/0/3 修改的 3 文件局部 fmt；CI 不跑 `cargo fmt --check`
- **推迟原因**：全量 fmt 体量灾难性（破坏 rebase、触发 build.rs 缓存失效、污染 blame）
- **恢复触发**：主动安排独立 PR + 团队通知 + 1 周缓冲期
- **完成后**：CI 加 `cargo fmt --check` step + `.git-blame-ignore-revs` 忽略 fmt commit

### cargo fmt 局部命令陷阱（Task 3 implementer C1 暴露）
- **位置**：`src-tauri/Cargo.toml:11`（`edition = "2021"`）+ 无 workspace 级 `rustfmt.toml`
- **现状**：
  - `rustfmt <file>` 单独跑报 E0670（rustfmt 默认 edition 2015，不识别 async fn，未透传项目 edition）
  - `cargo fmt -p claude-devtools -- <file>` 的 `-p` 仍会格式化整个包（导致工作区污染）
  - 正确但受限做法：`cargo fmt -p claude-devtools -- src/path/to/file.rs`（仍可能影响包内其他文件）
- **根因解决**：全量 fmt baseline 完成后（见上条），局部改动随提交自动 fmt，无需手动指定文件
- **恢复触发**：全量 cargo fmt PR 合入后本条自动失效，并入上条

### react-hooks v7 规则降级（Task 1 review minor note）
- **位置**：
  - `src/components/settings/SettingsView.tsx:32`（`set-state-in-effect`：effect 内 `setActiveSection(pendingSettingsSection)`）
  - `src/hooks/useClickOutside.ts:13`（`refs`：渲染期写 `callbackRef.current = onClickOutside`）
- **现状**：`eslint.config.js:42-43` 将 `react-hooks/set-state-in-effect` 和 `react-hooks/refs` 降级为 `warn`
- **推迟原因**：3e 第一批聚焦 CI 保护伞（lint 配置落地），不重构既有 hook 模式
- **恢复触发**：主动重构 effect→derived state / ref 写入移到 `useEffect` 后，恢复为 `error`
- **修复方案**：`SettingsView.tsx` 改为 `const activeSection = pendingSettingsSection ?? baseSection`；`useClickOutside.ts` 在 `useEffect` 内同步 ref

### eszett casefolding 测试被 ignore
- **位置**：`src-tauri/src/discovery/session_searcher.rs:962`，函数 `test_search_eszett_casefolding_unsupported`
- **根因**：pre-filter（`session_searcher.rs:462`，`entries.iter().any(|e| e.text.to_lowercase().contains(query))`）用 `to_lowercase().contains(query)` 短路；Rust `'ß'.to_lowercase() = "ß"`（无 SpecialCasing 展开）
- **推迟原因**：修复需在 pre-filter + collect_matches 两处加 casefolding，属业务逻辑改进，非 3e 工程基础设施
- **恢复触发**：用户反馈搜索德语内容受影响，或主动实现 Unicode SpecialCasing 支持
- **完成后**：移除 `#[ignore]`，测试重命名为 `test_search_eszett_casefolding_supported`

### 前端测试覆盖率为 0（CI test 步骤为占位）
- **位置**：`src/`（`find src -name "*.test.*" -o -name "*.spec.*"` 返回 0）+ `vite.config.ts:21` `passWithNoTests: true` + `.github/workflows/ci.yml:75-76` Frontend test 步骤
- **现状**：vitest v4 无测试文件时默认 exit 1；`passWithNoTests: true` 让 CI 的 "Frontend test" 步骤 exit 0——**当前是虚假绿灯**（永远通过但不验证任何东西）
- **推迟原因**：3e 第一批聚焦 CI 保护伞，不要求补前端测试
- **风险**：未来维护者可能误以为前端有测试覆盖
- **恢复触发**：3d Item 6（ConnectionStatusBadge 单测）或主动引入前端测试
- **完成后**：补至少 1 个 smoke test 后，可选移除 `passWithNoTests`（或保留，因有测试后该选项无副作用）

### tailwind.config.js ESM/CJS 潜在冲突
- **位置**：`tailwind.config.js`（CJS `module.exports`）vs `package.json: "type": "module"`
- **当前状态**：tailwindcss v3 通过 jiti 容忍，dev/build 实测可跑（Task 1 Step 8 验证）
- **风险**：tailwindcss v4 或工具链变化会崩
- **恢复触发**：升级 tailwindcss 或 build 报错时
- **修复方案**：改 `tailwind.config.cjs` + 更新 `postcss.config.cjs` 引用

## codex 第三轮拒绝项（v4 决策，用户目标=完善 SSH 功能，非工程基础设施）

以下 codex 第三轮提议经评估**不纳入本批**，记录于此供未来重新评估：

- **README.md 更新**（CI badge + 脚本说明 + prereqs）：单人项目无 onboarding 需求；SSH 稳定后 3f 文档批统一
- **CLAUDE.md / AGENTS.md 同步新脚本**：已 M 状态是 phase 3a 改动，推迟到 3f
- **.vscode/extensions.json + settings.json**：单人 IDE 偏好
- **rust-toolchain.toml pin 版本**：stable 足够，pin 增加维护负担
- **dependabot.yml**：单人项目手动升级；5 个 action 均主流，供应链风险低
- **CI step summary metrics**：YAGNI，CI 失败直接看日志
- **CI reporter 优化**（eslint --format=github-actions 等）：YAGNI
- **ROADMAP 转 GitHub issues 双轨**：ROADMAP.md 入库已够（grep 检索）
- **CONTRIBUTING.md / RUNBOOK.md**：单人项目暂无需求
- **eslint 所有规则降 warn**：~~项目实测无 `==` 违规~~ **修正（深度 review）**：实测 `src/` 有 **30 处**宽松 `== null`/`!= null`（合法的 null|undefined 双判断惯用法），通过 `eqeqeq ['error', 'always', { null: 'ignore' }]` 豁免。保留 error 级 eqeqeq 维护其他场景的严格相等门槛；null:ignore 是有意设计非疏漏

## pnpm/Node 版本不一致
- **位置**：`.github/workflows/release.yml:37,42`（pnpm 9 + Node 20）vs 本地 + CI（pnpm 10 + Node 22）
- **推迟原因**：release.yml 已能正确运行，修改它需独立验证
- **恢复触发**：release 失败或主动统一
- **修复方案**：release.yml 升 pnpm 10 + Node 22，或加 `packageManager` 字段统一

## release.yml 缺 --frozen-lockfile
- **位置**：`.github/workflows/release.yml:46`
- **推迟原因**：修改 release.yml 需独立验证
- **风险**：lockfile 不同步时 silent update（非阻塞但脏）
- **恢复触发**：主动加固发布流水线

## 推迟到 phase 3e 第二批

### GitNexus 索引自动更新（Item 3）
- **依赖**：3d 完成（CI 需有测试可跑）
- **恢复触发**：3d 完成

### pre-commit hooks（Item 4）
- **依赖**：3d 完成（新代码 lint warning 已批量修复）
- **前置**：先批量修 clippy warning（见上方）+ 全量 cargo fmt（见上方）
- **恢复触发**：3d 完成

## 推迟到 phase 4

### SSH 错误类型统一（Item 5）
- **位置**：`src-tauri/src/infrastructure/ssh_auth.rs` + `connect_flow.rs`
- **现状**：单一 `AuthError { message: String }`，不分 TCP/Auth/SFTP/ConfigMerge
- **推迟原因**：复杂度高，phase 3a `AuthTrace` 已部分缓解

### ProxyJump / ProxyCommand / compression / StrictHostKeyChecking
- **位置**：`src-tauri/src/infrastructure/ssh_connection/host_resolver.rs:182`（compression no 硬编码）
- **推迟原因**：企业内网友好性问题，需 plan 显式声明

### 多 agent candidate 并行（最坏 N×3s）
- **位置**：`src-tauri/src/infrastructure/ssh_auth.rs` `do_auth_agent_multi`
- **推迟原因**：实际 N ≤ 2 通常；企业环境 N 可能 4+

## 已完成

（暂无）
