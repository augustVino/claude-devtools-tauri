# Check-11/Check-12 架构差距修复设计

> 修复搜索模块（check-11）和通知模块（check-12）的 16 个已验证架构差距，
> 使 Tauri 实现与 Electron 功能严格对齐，同时保持代码架构质量。

## 设计原则

1. **复用而非重写**：搜索模块消费已有的 `jsonl_parser` / `message_classifier` / `content_sanitizer` 基础设施，不内联重新实现
2. **单一职责**：解析、分类、清洗、搜索各自独立，新增能力放入正确的模块
3. **消除重复**：AI buffer grouping 只在 classifier 实现一份，去重逻辑只在 NotificationManager 保留

## 阶段一：搜索管线重构

解决 5 个关联问题：H-1（JSON 路径错误）、I-1（缺少清洗管线）、M-4（AI buffer 缺失）、M-6（isMeta 检查缺失）、M-7（sessionTitle 硬编码）。

### 根因

`session_searcher.rs:318-438` 的 `extract_searchable_entries()` 内联了简化版的 JSONL 解析和消息分类逻辑，直接操作原始 JSON，未复用已有的 `jsonl_parser`、`message_classifier`、`content_sanitizer` 模块。这导致：
- JSON 字段路径错误（用户消息 `message` 是对象不是字符串，assistant 的 `content` 不在顶层）
- 无 AI buffer grouping（每条 assistant 消息独立处理）
- 无 isMeta 过滤（工具结果被当作普通用户消息）
- 无内容清洗（噪声标签出现在搜索结果中）
- 无 session title 提取

### 改动清单

#### 1.1 新增 `GroupedMessage` 枚举和 `group_ai_messages()` — `message_classifier.rs`

```rust
/// 将连续 AI 分类消息合并为一组
pub enum GroupedMessage<'a> {
    Single {
        category: MessageCategory,
        message: &'a ParsedMessage,
    },
    AiGroup {
        messages: Vec<&'a ParsedMessage>,
        group_id: String,  // "ai-{first_uuid}"
    },
}

pub fn group_ai_messages<'a>(
    classified: Vec<(MessageCategory, &'a ParsedMessage)>,
) -> Vec<GroupedMessage<'a>>
```

逻辑：遍历分类结果，连续 `MessageCategory::Ai` 消息收集到 buffer，遇到非 Ai 分类时 flush buffer 为一个 `AiGroup`。`group_id` 取 buffer 中首条消息的 UUID。

放在 `message_classifier` 而非 `session_searcher` 的原因：AI buffer grouping 是分类层面的概念，未来其他模块也可能需要。

#### 1.2 新增 `extract_session_title_from_parsed()` — `content_sanitizer.rs`

```rust
pub fn extract_session_title_from_parsed(messages: &[ParsedMessage]) -> Option<String>
```

接受 `&[ParsedMessage]` 输入的重载。内部逻辑：遍历消息找第一条 `is_real_user_message`（type=user, is_meta=false），提取其文本内容，截取前 500 字符。

当前 `extract_session_title` 接受 `Iterator<Item = &serde_json::Value>`（原始 JSON），新函数复用核心逻辑但接受类型化输入。如果可能，重构为共用内部实现。

#### 1.3 重写 `extract_searchable_entries()` — `session_searcher.rs`

删除内联解析逻辑（约 120 行），替换为管线调用：

```
原始 JSONL entries (Vec<serde_json::Value>)
  → parse_jsonl_content() → Vec<ParsedMessage>
  → deduplicate_by_request_id() → Vec<ParsedMessage>
  → classify_messages() → Vec<(MessageCategory, &ParsedMessage)>
  → group_ai_messages() → Vec<GroupedMessage>
  → 遍历 GroupedMessage:
      User → sanitize_display_content(text) → SearchableEntry
      AiGroup → 合并文本 → SearchableEntry (group_id = "ai-{first_uuid}")
      其他 → 跳过
  → extract_session_title_from_parsed() → 缓存 session title
```

生成的 `SearchableEntry` 保持现有结构不变（text, message_type, timestamp, group_id, item_type, message_uuid），`collect_matches_for_entry` 无需修改。

**关键变更**：
- `CacheEntry` 需要添加 `session_title: Option<String>` 字段
- `collect_matches_for_entry` 使用缓存的 `session_title` 而非硬编码 `"Untitled Session"`
- 删除内联的 `is_noise_only_user_message()` 函数（已被 `classify_messages` 取代）

#### 1.4 修正测试数据

当前 `session_searcher.rs` 中的测试使用构造的 JSONL 数据（`"message": "Hello world"` 字符串格式），与真实 JSONL 格式不匹配。需要修正测试数据为真实格式：

```json
{"type":"user","message":{"role":"user","content":"Hello world"},"uuid":"...","timestamp":"..."}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"response"}],"model":"..."},"uuid":"...","timestamp":"..."}
```

## 阶段二：搜索模块剩余修复

### 2.1 M-2 Subproject 过滤 — `session_searcher.rs`

在 `search_sessions()` 中注入 `SubprojectRegistry` 的 session filter：

- `SessionSearcher` 新增方法或参数接受 subproject session filter
- 遍历 `.jsonl` 文件时，如果 filter 存在，跳过不在 filter 集合中的 session
- 需要检查 `SubprojectRegistry` 是否已暴露获取 filter 的接口

### 2.2 M-5 HTTP search_all_projects — `http/routes/search.rs`

当前 `search_all_projects` 是 TODO stub，返回空结果。修复为调用已有的 `searcher.search_all_projects()`：

- 从 AppState 获取 SessionSearcher 实例
- 调用 `searcher.search_all_projects(query, max_results)`
- 返回结果（IPC 版本已有完整实现可参考）

### 2.3 L-1 max_results 上限 — `commands/search.rs` + `http/routes/search.rs`

将 `.min(100)` 改为 `.min(200)`，与 Electron 的 `coerceSearchMaxResults` 对齐。涉及两处：
- `src-tauri/src/commands/search.rs`
- `src-tauri/src/http/routes/search.rs`

### 2.4 L-2 阶段边界修复 — `session_searcher.rs`

`build_fast_search_stage_boundaries()` 两个问题：
1. 缺少最终边界（应 push `total_files`）
2. 缺少 `min(total_files, limit)` capping

修复：
```rust
fn build_fast_search_stage_boundaries(total_files: usize) -> Vec<usize> {
    let mut boundaries: Vec<usize> = SSH_FAST_SEARCH_STAGE_LIMITS
        .iter()
        .map(|&limit| limit.min(total_files))
        .filter(|&boundary| boundary > 0 && boundary < total_files)
        .collect();
    boundaries.dedup();
    if boundaries.last() != Some(&total_files) {
        boundaries.push(total_files);
    }
    boundaries
}
```

### 2.5 L-3 mtime 回退 — `session_searcher.rs`

将 `dirent.mtime_ms.unwrap_or(0)` 改为异步回退到 `fs_provider.stat()`：

```rust
let mtime = match dirent.mtime_ms {
    Some(ms) => ms,
    None => {
        match self.fs_provider.stat(&path).await {
            Ok(metadata) => metadata.mtime_ms.unwrap_or(0),
            Err(_) => 0,
        }
    }
};
```

注意：此处需要确认 `search_sessions` 中文件排序所在代码块是否已为 async 上下文。

## 阶段三：通知模块修复

### 3.1 M-8 TriggerManager apply_updates 遗漏字段 — `trigger_manager.rs`

在 `apply_updates()` 函数中添加 `repositoryIds` 和 `tokenType` 字段处理：

```rust
if let Some(Value::Array(ids)) = updates.get("repositoryIds") {
    // 解析为 Vec<String> 并更新
}
if let Some(Value::String(tt)) = updates.get("tokenType") {
    // 更新 tokenType 字段
}
```

需确认 `NotificationTrigger` 结构体是否已有这两个字段。如果没有，需先添加。

### 3.2 I-4 去重逻辑单点化 — `error_detector.rs` + `notification_manager.rs`

- 移除 `ErrorDetector::deduplicate_errors()` 中的去重逻辑
- `ErrorDetector` 返回原始错误列表（包含可能的重复）
- 去重完全由 `NotificationManager::add_error()` 负责
- 确认 `add_error()` 的去重逻辑（按 tool_use_id，优先保留 subagent 版本）完整正确

### 3.3 I-5 Regex 缓存 — `notification_manager.rs`

在 `NotificationManager` 中添加 regex 缓存：

```rust
struct NotificationManager {
    // ...existing fields...
    regex_cache: std::collections::HashMap<String, Option<Regex>>,
}

const MAX_REGEX_CACHE_SIZE: usize = 500;
```

`matches_ignored_regex()` 修改为：
1. 查缓存，命中直接返回
2. 未命中则调用 `create_safe_regex()`，结果（包括 `None`）写入缓存
3. 缓存超过上限时清空

### 3.4 L-4 Token 估算修正 — `tool_extraction.rs`

将 `(text.len() / 4).max(1)` 改为：

```rust
fn estimate_tokens(text: &str) -> u32 {
    if text.is_empty() { return 0; }
    ((text.len() as u32 + 3) / 4)  // 等价于 Math.ceil(len / 4)
}
```

### 3.5 L-5 Regex 长度限制收紧 — `regex_validation.rs`

将 `MAX_PATTERN_LENGTH` 从 `10_000` 改为 `100`，与 Electron 对齐。保持 500ms 超时方案不变。

## 测试策略

每个修复同步编写/更新单元测试：

1. **管线重构测试**：使用真实 JSONL 格式数据（对象形式的 `message`，数组形式的 `content`）
2. **AI buffer grouping 测试**：验证连续 assistant 消息合并为单条 searchable entry
3. **isMeta 过滤测试**：验证 meta 用户消息不产生搜索条目
4. **session title 测试**：验证从首条用户消息提取标题
5. **各独立修复**：针对性测试用例覆盖

## 影响范围

### 文件改动清单

| 文件 | 改动类型 |
|------|---------|
| `src-tauri/src/parsing/message_classifier.rs` | 新增 `GroupedMessage` + `group_ai_messages()` |
| `src-tauri/src/utils/content_sanitizer.rs` | 新增 `extract_session_title_from_parsed()` |
| `src-tauri/src/discovery/session_searcher.rs` | 重写 `extract_searchable_entries()`，修复 L-2/L-3，更新 `CacheEntry` |
| `src-tauri/src/http/routes/search.rs` | 实现 `search_all_projects` (M-5)，改 max_results 上限 (L-1) |
| `src-tauri/src/commands/search.rs` | 改 max_results 上限 (L-1) |
| `src-tauri/src/infrastructure/trigger_manager.rs` | 添加字段处理 (M-8) |
| `src-tauri/src/error/error_detector.rs` | 移除去重逻辑 (I-4) |
| `src-tauri/src/infrastructure/notification_manager.rs` | 移除重复去重 (I-4)，添加 regex 缓存 (I-5) |
| `src-tauri/src/analysis/tool_extraction.rs` | 修正 token 估算 (L-4) |
| `src-tauri/src/utils/regex_validation.rs` | 收紧长度限制 (L-5) |

### 无改动文件

前端代码无需改动——所有修复都是后端行为对齐，`SearchResult` 等返回类型不变。
