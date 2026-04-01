# Check-11/Check-12 架构差距修复实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复搜索和通知模块的 16 个架构差距，使 Tauri 与 Electron 功能严格对齐。

**Architecture:** 三阶段推进。阶段一重构搜索管线（复用 jsonl_parser/message_classifier/content_sanitizer，一次解决 5 个关联问题）；阶段二修复搜索模块 5 个独立问题；阶段三修复通知模块 5 个独立问题。

**Tech Stack:** Rust, Tauri v2, serde_json, regex, chrono, moka (LRU cache)

**Spec:** `docs/superpowers/specs/2026-04-01-check-11-12-architecture-gaps-fix-design.md`

---

## 阶段一：搜索管线重构（H-1 / I-1 / M-4 / M-6 / M-7）

### Task 1: 新增 `group_ai_messages()` — `message_classifier.rs`

**Files:**
- Modify: `src-tauri/src/parsing/message_classifier.rs`
- Modify: `src-tauri/src/types/domain.rs` (新增 `GroupedMessage` 枚举)

- [ ] **Step 1: 在 `domain.rs` 中添加 `GroupedMessage` 枚举**

在 `MessageCategory` 枚举定义之后（约 line 34 后）添加：

```rust
/// 将连续 AI 分类消息合并为一组后的分组结果。
///
/// 搜索模块使用此结构将同一流式响应的多条 assistant 消息合并为单个可搜索条目，
/// groupId 取首条消息的 UUID（与 Electron 的 `ai-${buffer[0].uuid}` 对齐）。
pub enum GroupedMessage<'a> {
    /// 非连续 AI 消息，独立保留
    Single {
        category: MessageCategory,
        message: &'a ParsedMessage,
    },
    /// 连续 AI 消息合并为一组
    AiGroup {
        messages: Vec<&'a ParsedMessage>,
        group_id: String,
    },
}
```

- [ ] **Step 2: 在 `message_classifier.rs` 中添加 `group_ai_messages()` 函数**

在文件末尾（tests 模块之前）添加：

```rust
/// 将分类结果中连续的 AI 消息合并为一组。
///
/// 遍历分类后的消息列表，收集连续 `MessageCategory::Ai` 消息到 buffer，
/// 遇到非 Ai 分类时 flush buffer 为一个 `GroupedMessage::AiGroup`。
/// `group_id` 取 buffer 中首条消息的 UUID，格式为 `"ai-{uuid}"`。
pub fn group_ai_messages<'a>(
    classified: Vec<(MessageCategory, &'a ParsedMessage)>,
) -> Vec<GroupedMessage<'a>> {
    let mut result = Vec::new();
    let mut ai_buffer: Vec<&'a ParsedMessage> = Vec::new();

    let flush_ai_buffer = |buf: &mut Vec<&'a ParsedMessage>, res: &mut Vec<GroupedMessage<'a>>| {
        if buf.is_empty() {
            return;
        }
        let group_id = format!("ai-{}", buf[0].uuid);
        res.push(GroupedMessage::AiGroup {
            messages: std::mem::take(buf),
            group_id,
        });
    };

    for (category, msg) in classified {
        if category == MessageCategory::Ai {
            ai_buffer.push(msg);
        } else {
            flush_ai_buffer(&mut ai_buffer, &mut result);
            result.push(GroupedMessage::Single { category, message: msg });
        }
    }
    flush_ai_buffer(&mut ai_buffer, &mut result);

    result
}
```

在文件顶部 imports 中添加：
```rust
use crate::types::domain::GroupedMessage;
```

- [ ] **Step 3: 编写 `group_ai_messages` 测试**

在 `message_classifier.rs` 的 tests 模块中添加：

```rust
#[test]
fn test_group_ai_messages_single_ai() {
    // 单条 AI 消息应合并为 AiGroup（含 1 条）
    let msgs = vec![
        make_parsed("u1", MessageType::User, "hello", false),
        make_parsed("a1", MessageType::Assistant, "response", false),
        make_parsed("u2", MessageType::User, "thanks", false),
    ];
    let classified = classify_messages(&msgs);
    let grouped = group_ai_messages(classified);

    assert_eq!(grouped.len(), 3);
    assert!(matches!(&grouped[0], GroupedMessage::Single { category: MessageCategory::User, .. }));
    assert!(matches!(&grouped[1], GroupedMessage::AiGroup { .. }));
    if let GroupedMessage::AiGroup { messages, group_id } = &grouped[1] {
        assert_eq!(messages.len(), 1);
        assert_eq!(group_id, "ai-a1");
    }
    assert!(matches!(&grouped[2], GroupedMessage::Single { category: MessageCategory::User, .. }));
}

#[test]
fn test_group_ai_messages_consecutive_ai_buffered() {
    // 连续多条 AI 消息应合并为一个 AiGroup
    let msgs = vec![
        make_parsed("u1", MessageType::User, "hello", false),
        make_parsed("a1", MessageType::Assistant, "part1", false),
        make_parsed("a2", MessageType::Assistant, "part2", false),
        make_parsed("a3", MessageType::Assistant, "part3", false),
        make_parsed("u2", MessageType::User, "thanks", false),
    ];
    let classified = classify_messages(&msgs);
    let grouped = group_ai_messages(classified);

    assert_eq!(grouped.len(), 3);
    if let GroupedMessage::AiGroup { messages, group_id } = &grouped[1] {
        assert_eq!(messages.len(), 3);
        assert_eq!(group_id, "ai-a1"); // 取首条 UUID
    }
}

#[test]
fn test_group_ai_messages_all_categories() {
    // HardNoise 消息应被跳过（classify 时已标记），不会出现在分类结果中
    // 实际上 classify_messages 返回所有消息的分类，只是 HardNoise 不应产生 searchable entry
    // group_ai_messages 不做过滤，仅做分组
    let msgs = vec![
        make_parsed("s1", MessageType::System, "system msg", false),
        make_parsed("u1", MessageType::User, "hello", false),
        make_parsed("a1", MessageType::Assistant, "response", false),
    ];
    let classified = classify_messages(&msgs);
    let grouped = group_ai_messages(classified);
    // System → HardNoise, User → User, Assistant → Ai
    assert_eq!(grouped.len(), 3);
}
```

注意：测试中使用的 `make_parsed` 辅助函数可能已存在于现有测试中。如不存在，需要添加一个构建 `ParsedMessage` 的 helper。

- [ ] **Step 4: 运行测试确认通过**

```bash
cd /Users/liepin/Documents/github/claude-devtools-tauri && cargo test -p claude-devtools-tauri message_classifier -- --nocapture
```

- [ ] **Step 5: 提交**

```bash
git add -f src-tauri/src/types/domain.rs src-tauri/src/parsing/message_classifier.rs
git commit -m "feat(search): add GroupedMessage enum and group_ai_messages() for AI buffer grouping (M-4)"
```

---

### Task 2: 新增 `extract_session_title_from_parsed()` — `content_sanitizer.rs`

**Files:**
- Modify: `src-tauri/src/utils/content_sanitizer.rs`

- [ ] **Step 1: 添加 `extract_session_title_from_parsed()` 函数**

在 `extract_session_title()` 函数之后（约 line 195 后）添加：

```rust
/// 从已解析的 `ParsedMessage` 列表中提取会话标题。
///
/// 逻辑与 `extract_session_title` 一致：查找第一条真实的用户消息文本，
/// 截取前 500 字符作为标题。使用 `jsonl_parser::extract_text_content` 提取纯文本。
pub fn extract_session_title_from_parsed(messages: &[ParsedMessage]) -> Option<String> {
    use crate::parsing::jsonl_parser::extract_text_content;
    use crate::parsing::message_classifier::is_real_user_message;

    for msg in messages {
        if is_real_user_message(msg) {
            let text = extract_text_content(msg);
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                let title = if trimmed.len() > 500 {
                    trimmed[..500].to_string()
                } else {
                    trimmed.to_string()
                };
                return Some(title);
            }
        }
    }
    None
}
```

在文件顶部 imports 中添加：
```rust
use crate::types::messages::ParsedMessage;
```

- [ ] **Step 2: 编写测试**

在 `content_sanitizer.rs` 的 tests 模块中添加：

```rust
#[test]
fn test_extract_session_title_from_parsed_basic() {
    let msgs = vec![
        ParsedMessage {
            uuid: "u1".into(),
            message_type: MessageType::User,
            content: serde_json::json!("Hello, this is my first message"),
            is_meta: false,
            ..Default::default()
        },
    ];
    let title = extract_session_title_from_parsed(&msgs);
    assert_eq!(title, Some("Hello, this is my first message".to_string()));
}

#[test]
fn test_extract_session_title_from_parsed_skips_meta() {
    let msgs = vec![
        ParsedMessage {
            uuid: "m1".into(),
            message_type: MessageType::User,
            content: serde_json::json!("meta content"),
            is_meta: true,
            ..Default::default()
        },
        ParsedMessage {
            uuid: "u1".into(),
            message_type: MessageType::User,
            content: serde_json::json!("real user message"),
            is_meta: false,
            ..Default::default()
        },
    ];
    let title = extract_session_title_from_parsed(&msgs);
    assert_eq!(title, Some("real user message".to_string()));
}

#[test]
fn test_extract_session_title_from_parsed_truncates() {
    let long_text = "a".repeat(600);
    let msgs = vec![ParsedMessage {
        uuid: "u1".into(),
        message_type: MessageType::User,
        content: serde_json::json!(long_text),
        is_meta: false,
        ..Default::default()
    }];
    let title = extract_session_title_from_parsed(&msgs).unwrap();
    assert_eq!(title.len(), 500);
}
```

注意：需确认 `ParsedMessage` 是否 derive 了 `Default`。如没有，需用完整字段构造。

- [ ] **Step 3: 运行测试确认通过**

```bash
cargo test -p claude-devtools-tauri content_sanitizer -- --nocapture
```

- [ ] **Step 4: 提交**

```bash
git add -f src-tauri/src/utils/content_sanitizer.rs
git commit -m "feat(search): add extract_session_title_from_parsed() for typed input (M-7)"
```

---

### Task 3: 重写 `extract_searchable_entries()` — `session_searcher.rs`

这是阶段一的核心改动，解决 H-1 / I-1 / M-4 / M-6 / M-7 五个问题。

**Files:**
- Modify: `src-tauri/src/discovery/session_searcher.rs`

- [ ] **Step 1: 更新 `CacheEntry` 结构体，添加 `session_title` 字段**

将 `CacheEntry`（line 29-33）从：
```rust
struct CacheEntry {
    mtime: u64,
    entries: Vec<SearchableEntry>,
}
```
改为：
```rust
struct CacheEntry {
    mtime: u64,
    entries: Vec<SearchableEntry>,
    session_title: Option<String>,
}
```

- [ ] **Step 2: 更新 `search_session_file()` 方法，传递 session_title**

修改 `search_session_file()`（line 263-315）使其从 cache entry 中获取 `session_title` 并传递给 `collect_matches_for_entry`：

1. 修改 `collect_matches_for_entry` 签名，添加 `session_title: &str` 参数：
```rust
fn collect_matches_for_entry(
    &self,
    entry: &SearchableEntry,
    query: &str,
    results: &mut Vec<SearchResult>,
    max_results: usize,
    project_id: &str,
    session_id: &str,
    session_title: &str,  // 新增
)
```

2. 在 `collect_matches_for_entry` 中将硬编码的 `"Untitled Session"`（line 476）替换为 `session_title.to_string()`

3. 在 `search_session_file` 中，从 cache entry 获取 session_title：
```rust
let (entries, session_title) = {
    let cache_key = file_path.to_string_lossy().to_string();
    if let Some(cached) = self.cache.get(&cache_key) {
        if cached.mtime == mtime {
            (cached.entries.clone(), cached.session_title.clone())
        } else {
            let (entries, title) = self.extract_searchable_entries(file_path)?;
            self.cache.insert(cache_key, CacheEntry { mtime, entries: entries.clone(), session_title: title.clone() });
            (entries, title)
        }
    } else {
        let (entries, title) = self.extract_searchable_entries(file_path)?;
        self.cache.insert(cache_key, CacheEntry { mtime, entries: entries.clone(), session_title: title.clone() });
        (entries, title)
    }
};
```

4. 将 `session_title` 传递给 `collect_matches_for_entry`：
```rust
self.collect_matches_for_entry(
    entry, query, &mut results, max_results,
    project_id, session_id,
    session_title.as_deref().unwrap_or("Untitled Session"),
);
```

- [ ] **Step 3: 重写 `extract_searchable_entries()` 返回类型和实现**

将返回类型从 `Result<Vec<SearchableEntry>, std::io::Error>` 改为 `Result<(Vec<SearchableEntry>, Option<String>), std::io::Error>`。

完整替换 `extract_searchable_entries()` 函数（line 318-438）为：

```rust
/// Extract searchable entries from a session file using the full parsing pipeline.
///
/// Pipeline: read → parse_jsonl_content → deduplicate_by_request_id →
/// classify_messages → group_ai_messages → sanitize → extract title
fn extract_searchable_entries(&self, file_path: &Path) -> Result<(Vec<SearchableEntry>, Option<String>), std::io::Error> {
    let content = self.fs_provider.read_file(file_path)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

    // 1. Parse JSONL → ParsedMessage (correctly handles message.content paths)
    let messages = crate::parsing::jsonl_parser::parse_jsonl_content(&content);

    // 2. Deduplicate streaming responses
    let messages = crate::parsing::jsonl_parser::deduplicate_by_request_id(&messages);

    // 3. Classify messages (includes isMeta filtering, sidechain, synthetic model detection)
    let classified = crate::parsing::message_classifier::classify_messages(&messages);

    // 4. Group consecutive AI messages into buffer groups
    let grouped = crate::parsing::message_classifier::group_ai_messages(classified);

    // 5. Extract session title from parsed messages
    let session_title = crate::utils::content_sanitizer::extract_session_title_from_parsed(&messages);

    // 6. Build searchable entries from grouped messages
    let mut entries = Vec::new();

    for group in &grouped {
        match group {
            GroupedMessage::Single { category: MessageCategory::User, message } => {
                let raw_text = crate::parsing::jsonl_parser::extract_text_content(message);
                let text = crate::utils::content_sanitizer::sanitize_display_content(&raw_text);
                if text.trim().is_empty() {
                    continue;
                }
                let timestamp = parse_timestamp(&message.timestamp);
                entries.push(SearchableEntry {
                    text,
                    message_type: "user".to_string(),
                    timestamp,
                    group_id: Some(format!("user-{}", message.uuid)),
                    item_type: Some("user".to_string()),
                    message_uuid: Some(message.uuid.clone()),
                });
            }
            GroupedMessage::AiGroup { messages: ai_msgs, group_id } => {
                // Merge all AI message texts into one searchable entry
                let mut combined_text = String::new();
                let mut last_timestamp: u64 = 0;
                let mut first_uuid: Option<String> = None;

                for msg in ai_msgs {
                    let text = crate::parsing::jsonl_parser::extract_text_content(msg);
                    if !text.trim().is_empty() {
                        if !combined_text.is_empty() {
                            combined_text.push('\n');
                        }
                        combined_text.push_str(&text);
                    }
                    let ts = parse_timestamp(&msg.timestamp);
                    if ts > last_timestamp {
                        last_timestamp = ts;
                    }
                    if first_uuid.is_none() {
                        first_uuid = Some(msg.uuid.clone());
                    }
                }

                if combined_text.trim().is_empty() {
                    continue;
                }

                entries.push(SearchableEntry {
                    text: combined_text,
                    message_type: "assistant".to_string(),
                    timestamp: last_timestamp,
                    group_id: Some(group_id.clone()),
                    item_type: Some("ai".to_string()),
                    message_uuid: first_uuid,
                });
            }
            // HardNoise, System, Compact — skip
            _ => continue,
        }
    }

    Ok((entries, session_title))
}
```

添加辅助函数（在 impl 块外）：
```rust
/// Parse RFC3339 timestamp to milliseconds.
fn parse_timestamp(ts: &str) -> u64 {
    chrono::DateTime::parse_from_rfc3339(ts)
        .map(|dt| dt.timestamp_millis() as u64)
        .unwrap_or(0)
}
```

- [ ] **Step 4: 添加必要的 imports**

在 `session_searcher.rs` 顶部添加：
```rust
use crate::types::domain::GroupedMessage;
```

- [ ] **Step 5: 删除已废弃的内联函数**

删除 `is_noise_only_user_message()` 函数（line 498-521），因为噪声过滤已由 `classify_messages` + `sanitize_display_content` 接管。

同时删除旧的 noise filter 测试（`test_noise_filter_*` 系列 7 个测试）。

- [ ] **Step 6: 修正测试数据格式**

将现有测试中的 JSONL 数据修正为真实格式。例如 `test_search_with_match`：

```rust
#[test]
fn test_search_with_match() {
    let (temp_dir, mut searcher) = setup_test_env();

    let project_dir = temp_dir.path().join("projects").join("-Users-test-project");
    fs::create_dir_all(&project_dir).unwrap();

    let session_content = r#"{"type":"user","message":{"role":"user","content":"Hello world this is a test"},"uuid":"u1","timestamp":"2024-01-01T00:00:00Z"}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"This is the assistant response with test keyword"}],"model":"claude-3"},"uuid":"a1","timestamp":"2024-01-01T00:01:00Z"}
"#;
    fs::write(project_dir.join("session-123.jsonl"), session_content).unwrap();

    let result = searcher.search_sessions("-Users-test-project", "test", 50);
    assert!(result.total_matches > 0);
    assert_eq!(result.sessions_searched, 1);
    // 验证 session_title 被提取
    assert_ne!(result.results[0].session_title, "Untitled Session");
}
```

同样修正 `test_search_max_results`：
```rust
#[test]
fn test_search_max_results() {
    let (temp_dir, mut searcher) = setup_test_env();

    let project_dir = temp_dir.path().join("projects").join("-Users-test-project");
    fs::create_dir_all(&project_dir).unwrap();

    let session_content = r#"{"type":"user","message":{"role":"user","content":"test test test test test"},"uuid":"u1","timestamp":"2024-01-01T00:00:00Z"}
"#;
    fs::write(project_dir.join("session-123.jsonl"), session_content).unwrap();

    let result = searcher.search_sessions("-Users-test-project", "test", 2);
    assert!(result.results.len() <= 2);
}
```

- [ ] **Step 7: 添加新测试覆盖关键场景**

```rust
#[test]
fn test_search_ai_buffer_grouping() {
    let (temp_dir, mut searcher) = setup_test_env();

    let project_dir = temp_dir.path().join("projects").join("-Users-test-project");
    fs::create_dir_all(&project_dir).unwrap();

    // 连续 3 条 assistant 消息应合并为 1 个 searchable entry
    let session_content = r#"{"type":"user","message":{"role":"user","content":"hello"},"uuid":"u1","timestamp":"2024-01-01T00:00:00Z"}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"response part one keyword"}],"model":"claude-3"},"uuid":"a1","timestamp":"2024-01-01T00:01:00Z"}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"response part two keyword"}],"model":"claude-3"},"uuid":"a2","timestamp":"2024-01-01T00:01:01Z"}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"response part three"}],"model":"claude-3"},"uuid":"a3","timestamp":"2024-01-01T00:01:02Z"}
"#;
    fs::write(project_dir.join("session-abc.jsonl"), session_content).unwrap();

    let result = searcher.search_sessions("-Users-test-project", "keyword", 50);
    // 两条匹配都应在同一个 AI group 中，group_id = "ai-a1"
    assert!(result.total_matches >= 2);
    assert!(result.results.iter().all(|r| r.group_id.as_deref() == Some("ai-a1")));
}

#[test]
fn test_search_ismeta_filtered() {
    let (temp_dir, mut searcher) = setup_test_env();

    let project_dir = temp_dir.path().join("projects").join("-Users-test-project");
    fs::create_dir_all(&project_dir).unwrap();

    // meta 用户消息（工具结果）不应出现在搜索中
    let session_content = r#"{"type":"user","message":{"role":"user","content":"tool result with keyword"},"uuid":"m1","timestamp":"2024-01-01T00:00:00Z","isMeta":true}
{"type":"user","message":{"role":"user","content":"real user message"},"uuid":"u1","timestamp":"2024-01-01T00:01:00Z"}
"#;
    fs::write(project_dir.join("session-meta.jsonl"), session_content).unwrap();

    let result = searcher.search_sessions("-Users-test-project", "keyword", 50);
    // meta 消息不应被搜索到
    assert_eq!(result.total_matches, 0);
}

#[test]
fn test_search_sanitized_content() {
    let (temp_dir, mut searcher) = setup_test_env();

    let project_dir = temp_dir.path().join("projects").join("-Users-test-project");
    fs::create_dir_all(&project_dir).unwrap();

    // 包含噪声标签的用户消息应被清洗后再搜索
    let session_content = r#"{"type":"user","message":{"role":"user","content":"<system-reminder>rules here</system-reminder>Please findme help"},"uuid":"u1","timestamp":"2024-01-01T00:00:00Z"}
"#;
    fs::write(project_dir.join("session-sanitize.jsonl"), session_content).unwrap();

    let result = searcher.search_sessions("-Users-test-project", "findme", 50);
    assert!(result.total_matches > 0);
    // context 中不应包含噪声标签
    assert!(!result.results[0].context.contains("<system-reminder>"));
}
```

- [ ] **Step 8: 运行全部搜索测试确认通过**

```bash
cargo test -p claude-devtools-tauri session_searcher -- --nocapture
```

- [ ] **Step 9: 提交**

```bash
git add -f src-tauri/src/discovery/session_searcher.rs
git commit -m "fix(search): rewrite extract_searchable_entries to use parsing pipeline (H-1/I-1/M-4/M-6/M-7)"
```

---

## 阶段二：搜索模块剩余修复

### Task 4: 修复 `build_fast_search_stage_boundaries` (L-2)

**Files:**
- Modify: `src-tauri/src/discovery/session_searcher.rs:526-532`

- [ ] **Step 1: 编写失败测试**

在 session_searcher 的 tests 模块中添加：

```rust
#[test]
fn test_fast_search_stage_boundaries_includes_final() {
    // total_files = 50, limits = [40, 140, 320]
    // 期望: [40, 50] (min capping + final push)
    let boundaries = build_fast_search_stage_boundaries(50);
    assert_eq!(boundaries, vec![40, 50]);
}

#[test]
fn test_fast_search_stage_boundaries_large() {
    // total_files = 500, limits = [40, 140, 320]
    // 期望: [40, 140, 320, 500] (all limits + final push)
    let boundaries = build_fast_search_stage_boundaries(500);
    assert_eq!(boundaries, vec![40, 140, 320, 500]);
}

#[test]
fn test_fast_search_stage_boundaries_small() {
    // total_files = 10, limits = [40, 140, 320]
    // 期望: [10] (所有 limits >= total_files, 只有 final push)
    let boundaries = build_fast_search_stage_boundaries(10);
    assert_eq!(boundaries, vec![10]);
}
```

注意：`build_fast_search_stage_boundaries` 当前是私有函数。测试在同一个模块中可以直接调用，或需在 `mod tests` 中通过 `use super::*` 访问。

- [ ] **Step 2: 运行测试确认失败**

```bash
cargo test -p claude-devtools-tauri test_fast_search_stage_boundaries -- --nocapture
```

预期 FAIL（当前实现不包含 final boundary）。

- [ ] **Step 3: 修复函数**

将 `build_fast_search_stage_boundaries()`（line 526-532）替换为：

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

- [ ] **Step 4: 运行测试确认通过**

```bash
cargo test -p claude-devtools-tauri test_fast_search_stage_boundaries -- --nocapture
```

- [ ] **Step 5: 提交**

```bash
git add -f src-tauri/src/discovery/session_searcher.rs
git commit -m "fix(search): add final boundary and min capping to stage boundaries (L-2)"
```

---

### Task 5: 修复 mtime 回退为 stat 调用 (L-3)

**Files:**
- Modify: `src-tauri/src/discovery/session_searcher.rs:132`

- [ ] **Step 1: 修改 mtime 提取逻辑**

将 line 132 从：
```rust
let mtime = dirent.mtime_ms.unwrap_or(0);
```
改为：
```rust
let mtime = match dirent.mtime_ms {
    Some(ms) => ms,
    None => {
        let path = project_path.join(&dirent.name);
        self.fs_provider.stat(&path).map(|m| m.mtime_ms).unwrap_or(0)
    }
};
```

注意：`path` 变量在后续 line 133 也用到（`let path = project_path.join(&dirent.name)`），需要合并为一次创建。将 line 133 的 `let path = ...` 移除（已在 mtime 分支中创建）。

完整替换 loop 内的 mtime + path 逻辑（line 127-134）为：
```rust
for dirent in entries {
    if !dirent.is_file || !dirent.name.ends_with(".jsonl") {
        continue;
    }

    let path = project_path.join(&dirent.name);
    let mtime = match dirent.mtime_ms {
        Some(ms) => ms,
        None => self.fs_provider.stat(&path).map(|m| m.mtime_ms).unwrap_or(0),
    };
    session_files.push((dirent.name, path, mtime));
}
```

- [ ] **Step 2: 运行测试确认不破坏现有功能**

```bash
cargo test -p claude-devtools-tauri session_searcher -- --nocapture
```

- [ ] **Step 3: 提交**

```bash
git add -f src-tauri/src/discovery/session_searcher.rs
git commit -m "fix(search): fallback to fs_provider.stat() for missing mtime (L-3)"
```

---

### Task 6: 修复 max_results 上限 (L-1)

**Files:**
- Modify: `src-tauri/src/commands/search.rs:25,57`
- Modify: `src-tauri/src/http/routes/search.rs:25,58`

- [ ] **Step 1: 修改 `commands/search.rs` 两处上限**

将 line 25 从：
```rust
let max = max_results.unwrap_or(50).min(100).max(1);
```
改为：
```rust
let max = max_results.unwrap_or(50).min(200).max(1);
```

将 line 57 做相同修改。

- [ ] **Step 2: 修改 `http/routes/search.rs` 两处上限**

将 line 25 从：
```rust
let max = max_results.unwrap_or(50).min(100).max(1);
```
改为：
```rust
let max = max_results.unwrap_or(50).min(200).max(1);
```

将 line 58 做相同修改（`let _max = ...`）。

- [ ] **Step 3: 运行编译确认无错误**

```bash
cargo check
```

- [ ] **Step 4: 提交**

```bash
git add -f src-tauri/src/commands/search.rs src-tauri/src/http/routes/search.rs
git commit -m "fix(search): align max_results cap to 200 matching Electron (L-1)"
```

---

### Task 7: 实现 HTTP `search_all_projects` (M-5)

**Files:**
- Modify: `src-tauri/src/http/routes/search.rs:49-81`

- [ ] **Step 1: 重写 `search_all_projects` 路由处理函数**

将 line 49-81 替换为：

```rust
/// 搜索所有项目中的会话。
///
/// GET /api/search?q=&maxResults=
pub async fn search_all_projects(
    State(state): State<HttpState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<SearchSessionsResult>, (StatusCode, Json<super::ErrorResponse>)> {
    let query = params.get("q").cloned().unwrap_or_default();
    let max_results = params
        .get("maxResults")
        .and_then(|v| v.parse::<u32>().ok());

    let max = max_results.unwrap_or(50).min(200).max(1);

    if query.trim().is_empty() {
        return Ok(Json(SearchSessionsResult {
            results: Vec::new(),
            total_matches: 0,
            sessions_searched: 0,
            query,
            is_partial: None,
        }));
    }

    // TODO: Wrap in tokio::task::spawn_blocking to avoid blocking the async runtime
    let mut searcher = state
        .searcher
        .lock()
        .map_err(|e| error_json(e.to_string()))?;
    Ok(Json(searcher.search_all_projects(&query, max)))
}
```

注意：`State(_state)` 改为 `State(state)` 以使用 searcher。`max` 从 `_max` 改为 `max`。

- [ ] **Step 2: 运行编译确认无错误**

```bash
cargo check
```

- [ ] **Step 3: 提交**

```bash
git add -f src-tauri/src/http/routes/search.rs
git commit -m "fix(search): implement HTTP search_all_projects route (M-5)"
```

---

### Task 8: 添加 Subproject 过滤 (M-2)

**Files:**
- Modify: `src-tauri/src/discovery/session_searcher.rs`
- Modify: `src-tauri/src/commands/search.rs`

**前置说明**：`ProjectScanner` 不持有 `SubprojectRegistry` 引用（已确认）。`SubprojectRegistry` 是独立模块（`discovery/subproject_registry.rs`）。
方案：在 `SessionSearcher` 中添加 `Option<Arc<SubprojectRegistry>>` 字段。

- [ ] **Step 1: 在 `SessionSearcher` 结构体中添加 subproject 字段**

在 `SessionSearcher` 的 `project_scanner` 字段之后添加：

```rust
/// Subproject 过滤注册表，用于搜索时过滤子项目会话。
subproject_registry: Option<Arc<SubprojectRegistry>>,
```

添加 import：
```rust
use crate::discovery::SubprojectRegistry;
```

- [ ] **Step 2: 更新 `SessionSearcher::new()` 构造函数**

添加 `subproject_registry` 参数：

```rust
pub fn new(
    projects_dir: PathBuf,
    todos_dir: PathBuf,
    fs_provider: Arc<dyn FsProvider>,
    subproject_registry: Option<Arc<SubprojectRegistry>>,
) -> Self {
    Self {
        projects_dir,
        fs_provider,
        cache: moka::sync::Cache::builder()
            .max_capacity(CACHE_MAX_CAPACITY)
            .build(),
        project_scanner: ProjectScanner::with_paths(
            projects_dir.clone(),
            todos_dir,
            fs_provider.clone(),
        ),
        subproject_registry,
    }
}
```

- [ ] **Step 3: 更新所有 `SessionSearcher::new()` 调用点**

在以下位置添加 `None` 参数：
- `commands/search.rs` 的 `create_searcher_state()`：`SessionSearcher::new(projects_dir, todos_dir, fs_provider, None)`
- `session_searcher.rs` 的 tests 中 `setup_test_env()`：`SessionSearcher::new(projects_dir, todos_dir, Arc::new(LocalFsProvider::new()), None)`

- [ ] **Step 4: 在 `search_sessions()` 中添加 subproject 过滤**

在文件排序后、搜索循环之前（约 line 145 后），添加：

```rust
// Subproject filtering: skip sessions not in the current project's filter
let session_filter = self.subproject_registry
    .as_ref()
    .and_then(|reg| reg.get_session_filter(project_id).cloned());
```

在搜索循环内，在 `let session_id = ...` 之后添加：
```rust
// Skip sessions not belonging to this subproject
if let Some(ref filter) = session_filter {
    if !filter.contains(&session_id) {
        continue;
    }
}
```

- [ ] **Step 5: 运行测试**

```bash
cargo test -p claude-devtools-tauri session_searcher -- --nocapture
```

- [ ] **Step 6: 提交**

```bash
git add -f src-tauri/src/discovery/session_searcher.rs src-tauri/src/commands/search.rs
git commit -m "feat(search): add subproject session filtering (M-2)"
```

---

## 阶段三：通知模块修复

### Task 9: 修正 token 估算 (L-4)

**Files:**
- Modify: `src-tauri/src/analysis/tool_extraction.rs:89`

- [ ] **Step 1: 修改 `estimate_tokens` 函数**

将 line 89 从：
```rust
(text.len() / 4).max(1)
```
改为：
```rust
if text.is_empty() { return 0; }
(text.len() + 3) / 4
```

完整的 `estimate_tokens` 函数变为：
```rust
pub fn estimate_tokens(content: &serde_json::Value) -> usize {
    let text = match content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            serde_json::to_string(content).unwrap_or_default()
        }
        _ => content.to_string(),
    };

    if text.is_empty() { return 0; }
    (text.len() + 3) / 4  // 等价于 Math.ceil(len / 4)
}
```

- [ ] **Step 2: 更新空内容测试断言**

找到测试 `test_estimate_tokens_empty`（或类似名称），将断言从 `assert_eq!(tokens, 1)` 改为 `assert_eq!(tokens, 0)`。

- [ ] **Step 3: 运行测试**

```bash
cargo test -p claude-devtools-tauri tool_extraction -- --nocapture
```

- [ ] **Step 4: 提交**

```bash
git add -f src-tauri/src/analysis/tool_extraction.rs
git commit -m "fix(notify): use ceiling division for token estimation, empty returns 0 (L-4)"
```

---

### Task 10: 收紧 regex 长度限制 (L-5)

**Files:**
- Modify: `src-tauri/src/utils/regex_validation.rs:10`

- [ ] **Step 1: 修改 `MAX_PATTERN_LENGTH` 常量**

将 line 10 从：
```rust
const MAX_PATTERN_LENGTH: usize = 10_000;
```
改为：
```rust
const MAX_PATTERN_LENGTH: usize = 100;
```

- [ ] **Step 2: 更新受影响的测试**

检查 `regex_validation.rs` 的测试中是否有使用超过 100 字符 pattern 的测试用例。如果有，需更新这些用例使用更短的 pattern。

运行测试确认：
```bash
cargo test -p claude-devtools-tauri regex_validation -- --nocapture
```

- [ ] **Step 3: 提交**

```bash
git add -f src-tauri/src/utils/regex_validation.rs
git commit -m "fix(notify): tighten regex pattern length limit to 100 matching Electron (L-5)"
```

---

### Task 11: 添加 regex 缓存 (I-5)

**Files:**
- Modify: `src-tauri/src/infrastructure/notification_manager.rs`

- [ ] **Step 1: 在 `NotificationManager` 结构体中添加缓存字段**

在 `last_shown_error` 字段之后（line 65 后）添加：

```rust
/// Regex 编译缓存：pattern → compiled regex (or None if invalid).
regex_cache: Arc<Mutex<HashMap<String, Option<regex::Regex>>>>,
```

- [ ] **Step 2: 在构造函数中初始化缓存**

在 `NotificationManager::new()` 中添加初始化：
```rust
regex_cache: Arc::new(Mutex::new(HashMap::new())),
```

- [ ] **Step 3: 修改 `matches_ignored_regex()` 使用缓存**

将 `matches_ignored_regex()`（line 524-544）替换为：

```rust
fn matches_ignored_regex(&self, error: &DetectedError) -> bool {
    let config = self.config_manager.get_config();

    if config.notifications.ignored_regex.is_empty() {
        return false;
    }

    for pattern in &config.notifications.ignored_regex {
        let case_insensitive = format!("(?i){}", pattern);

        // 查缓存
        {
            let cache = self.regex_cache.lock().ok();
            if let Some(guard) = cache {
                if let Some(cached) = guard.get(&case_insensitive) {
                    if let Some(ref re) = cached {
                        if re.is_match(&error.message) {
                            return true;
                        }
                    }
                    continue; // cached as None (invalid regex)
                }
            }
        }

        // 未命中缓存，编译并缓存
        let compiled = crate::utils::regex_validation::create_safe_regex(&case_insensitive);
        let is_match = compiled.as_ref().map_or(false, |re| re.is_match(&error.message));

        if let Ok(mut cache) = self.regex_cache.lock() {
            // 缓存超限时清空
            if cache.len() >= 500 {
                cache.clear();
            }
            cache.insert(case_insensitive, compiled);
        }

        if is_match {
            return true;
        }
    }

    false
}
```

注意：`HashMap` 和 `Regex` 已在文件 imports 中（line 14-19），无需重复添加。

- [ ] **Step 4: 运行测试**

```bash
cargo test -p claude-devtools-tauri notification_manager -- --nocapture
```

- [ ] **Step 5: 提交**

```bash
git add -f src-tauri/src/infrastructure/notification_manager.rs
git commit -m "perf(notify): add regex compilation cache for ignored patterns (I-5)"
```

---

### Task 12: 添加 `repositoryIds` 和 `tokenType` 到 apply_updates (M-8)

**Files:**
- Modify: `src-tauri/src/infrastructure/trigger_manager.rs:401-445`

- [ ] **Step 1: 在 `apply_updates()` 中添加两个遗漏字段**

在 `apply_updates()` 函数末尾（line 444 `// 注意: isBuiltin 被有意忽略` 之前）添加：

```rust
    if let Some(repository_ids) = updates.get("repositoryIds").and_then(|v| v.as_array()) {
        trigger.repository_ids = Some(
            repository_ids
                .iter()
                .filter_map(|p| p.as_str().map(String::from))
                .collect(),
        );
    }
    if let Some(token_type) = updates.get("tokenType").and_then(|v| v.as_str()) {
        if let Ok(tt) = serde_json::from_value(serde_json::json!(token_type)) {
            trigger.token_type = Some(tt);
        }
    }
```

- [ ] **Step 2: 编写测试**

在 trigger_manager 的 tests 模块中添加：

```rust
#[test]
fn test_apply_updates_repository_ids() {
    let mut trigger = default_triggers()[0].clone();
    let updates = serde_json::json!({
        "repositoryIds": ["repo1", "repo2"]
    });
    apply_updates(&mut trigger, &updates);
    assert_eq!(trigger.repository_ids, Some(vec!["repo1".to_string(), "repo2".to_string()]));
}

#[test]
fn test_apply_updates_token_type() {
    let mut trigger = default_triggers()[0].clone();
    let updates = serde_json::json!({
        "tokenType": "Output"
    });
    apply_updates(&mut trigger, &updates);
    assert_eq!(trigger.token_type, Some(TriggerTokenType::Output));
}
```

注意：需从 tests 的 imports 中确保 `apply_updates` 和 `TriggerTokenType` 可用。

- [ ] **Step 3: 运行测试**

```bash
cargo test -p claude-devtools-tauri trigger_manager -- --nocapture
```

- [ ] **Step 4: 提交**

```bash
git add -f src-tauri/src/infrastructure/trigger_manager.rs
git commit -m "fix(notify): add repositoryIds and tokenType to apply_updates (M-8)"
```

---

### Task 13: 移除 ErrorDetector 中的重复去重 (I-4)

**Files:**
- Modify: `src-tauri/src/error/error_detector.rs:217-242`
- Modify: `src-tauri/src/infrastructure/notification_manager.rs` (确认去重逻辑)

- [ ] **Step 1: 确认 `NotificationManager::add_error()` 去重逻辑完整**

已确认（spec review 中验证过）：`add_error()`（line 157-185）按 `tool_use_id` 去重，优先保留 subagent 版本。逻辑与 `ErrorDetector::deduplicate_errors()` 完全等价。可以安全移除 ErrorDetector 中的去重。

- [ ] **Step 2: 简化 `detect_errors()` 中的调用**

在 `detect_errors()` 方法中，找到调用 `Self::deduplicate_errors(results)` 的位置，将其替换为直接返回 `results`：

```rust
// 去重已由 NotificationManager::add_error() 负责
results
```

- [ ] **Step 3: 删除 `deduplicate_errors()` 方法**

删除 `deduplicate_errors()` 函数（line 217-242）。

- [ ] **Step 4: 更新受影响的测试**

需要处理以下测试：

1. **删除 6 个 `deduplicate_errors` 单元测试**：
   - `test_deduplicate_errors_no_duplicates` (line 568)
   - `test_deduplicate_errors_removes_duplicates` (line 615)
   - `test_deduplicate_errors_keeps_errors_without_tool_use_id` (line 663)
   - `test_deduplicate_errors_empty` (line 711)
   - `test_deduplicate_errors_prefers_subagent_version` (line 717)
   - `test_deduplicate_errors_keeps_existing_subagent_over_non_subagent` (line 768)

2. **更新集成测试 `test_detect_errors_deduplicates_by_tool_use_id`** (line 497)：
   移除去重后，`detect_errors()` 对同一 tool_use_id 的两个 trigger 会返回 2 条错误（而非之前的 1 条去重结果）。
   将断言 `assert_eq!(errors.len(), 1)` 改为 `assert_eq!(errors.len(), 2)`。

去重行为已由 `NotificationManager` 的测试覆盖（`test_add_error_dedup_*` 系列测试）。

```bash
cargo test -p claude-devtools-tauri error_detector -- --nocapture
```

- [ ] **Step 5: 提交**

```bash
git add -f src-tauri/src/error/error_detector.rs
git commit -m "refactor(notify): remove duplicate dedup from ErrorDetector, rely on NotificationManager (I-4)"
```

---

## 最终验证

### Task 14: 全量编译和测试

- [ ] **Step 1: 全量编译**

```bash
cargo build 2>&1 | head -50
```

- [ ] **Step 2: 运行所有受影响模块的测试**

```bash
cargo test -p claude-devtools-tauri -- --nocapture 2>&1 | tail -100
```

- [ ] **Step 3: 确认所有测试通过后，查看完整 diff**

```bash
git diff HEAD~14  # 14 个 task 的提交
```

- [ ] **Step 4: 最终合并提交（如需要）**

不合并——保持每个 task 的独立提交，便于 review 和回滚。
