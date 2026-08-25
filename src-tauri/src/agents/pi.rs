//! Pi（pi coding agent）适配器。
//!
//! 数据源：`~/.pi/agent/sessions/{目录编码}/{timestamp}_{sessionId}.jsonl`。
//!
//! # 格式要点（2026-08 实测，version 3）
//!
//! - 首行 `{type:"session", version, id, timestamp, cwd, tools}`：cwd 在首行，
//!   **权威**——目录名是 pi 自家编码（与 Claude 的编码规则不同且有损，
//!   一律不从目录名解码，每个文件读自己的首行）；
//! - 消息行 `{type:"message", id, parentId, timestamp, message:{role, model?,
//!   usage?, content:[blocks]}}`，行 `id` 唯一（直接作 ParsedMessage.uuid，
//!   保证 chunk id `{type}-{uuid}` 稳定）；
//! - user 的 content 为 `[{type:"text"}]`（与 Claude 块同构，原样透传）；
//! - assistant 的 blocks：`text`（同构）/ `thinking {thinking, thinkingSignature}`
//!   / `toolCall {id, name, arguments:对象}`；
//! - `toolResult` 是**独立 role** 的行：`{role:"toolResult", toolCallId,
//!   toolName, content:[text块], isError}`，映射为 Claude 协议的 user 消息内
//!   `tool_result` 块（`is_meta=true`，下游按 internal_user 折叠，与 Claude
//!   的 tool result 载体形态一致）；
//! - usage：`{input, output, cacheRead, cacheWrite, reasoning, totalTokens,
//!   cost}`（reasoning 无对应字段，忽略）；
//! - 已知噪声行：`model_change` / `thinking_level_change` / `custom`
//!   （`custom` 含 `subagents:record` 子代理记录，暂不解析）。
//!
//! # 与 Claude 管线的能力差异（如实声明）
//!
//! - 无 git 分支、无 todos、无 CLAUDE.md 生态文件（前端对应面板自然空态）；
//! - 无 requestId 流式重复（无需去重）；
//! - `custom: subagents:record` 未接入 → has_subagents 恒 false。

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::agents::{path_has_components, AgentAdapter, AgentSessionEntry};
use crate::infrastructure::fs_provider::FsProvider;
use crate::parsing::{extract_tool_calls, extract_tool_results};
use crate::types::domain::{AgentKind, MessageType, Session, SessionMetadataLevel};
use crate::types::jsonl::UsageMetadata;
use crate::types::messages::ParsedMessage;
use crate::utils::encode_path;

pub struct PiAdapter;

impl PiAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PiAdapter {
    fn default() -> Self {
        Self::new()
    }
}

/// 文件名 stem `{timestamp}_{sessionId}` → sessionId；无 `_` 时整个 stem 兜底。
fn session_id_from_stem(stem: &str) -> String {
    stem.rsplit('_').next().unwrap_or(stem).to_string()
}

/// 公共字段构造器：三个 role 分支共享，避免 13 个字段逐字复制导致分叉。
fn pi_msg(
    uuid: String,
    parent_id: Option<&str>,
    ty: MessageType,
    role: &str,
    timestamp: String,
    session_cwd: &Option<String>,
) -> ParsedMessage {
    ParsedMessage {
        uuid,
        parent_uuid: parent_id.map(String::from),
        message_type: ty,
        timestamp,
        role: Some(role.to_string()),
        content: Value::Null,
        usage: None,
        model: None,
        cwd: session_cwd.clone(),
        git_branch: None,
        agent_id: None,
        is_sidechain: false,
        is_meta: false,
        user_type: None,
        tool_calls: vec![],
        tool_results: vec![],
        source_tool_use_id: None,
        source_tool_assistant_uuid: None,
        tool_use_result: None,
        is_compact_summary: None,
        request_id: None,
    }
}

/// assistant blocks → Claude 块协议。
fn convert_assistant_blocks(content: &Value) -> Value {
    let Some(arr) = content.as_array() else {
        return content.clone();
    };
    let converted: Vec<Value> = arr
        .iter()
        .map(|b| match b.get("type").and_then(|t| t.as_str()) {
            Some("thinking") => serde_json::json!({
                "type": "thinking",
                "thinking": b.get("thinking").and_then(|v| v.as_str()).unwrap_or(""),
                "signature": b.get("thinkingSignature").and_then(|v| v.as_str()).unwrap_or(""),
            }),
            Some("toolCall") => serde_json::json!({
                "type": "tool_use",
                "id": b.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                "name": b.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                "input": b.get("arguments").cloned().unwrap_or(Value::Null),
            }),
            // text 等其余块与 Claude 协议同构，原样透传
            _ => b.clone(),
        })
        .collect();
    Value::Array(converted)
}

/// pi usage → Claude UsageMetadata（字段名映射；reasoning 无对应字段忽略）。
fn convert_usage(u: &Value) -> Option<UsageMetadata> {
    if u.is_null() {
        return None;
    }
    let get = |k: &str| u.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
    Some(UsageMetadata {
        input_tokens: get("input"),
        output_tokens: get("output"),
        cache_read_input_tokens: u.get("cacheRead").and_then(|v| v.as_u64()),
        cache_creation_input_tokens: u.get("cacheWrite").and_then(|v| v.as_u64()),
    })
}

/// pi JSONL 内容 → ParsedMessage（见模块文档的映射表）。
pub(crate) fn parse_pi_content(content: &str) -> Vec<ParsedMessage> {
    let mut out: Vec<ParsedMessage> = Vec::new();
    let mut session_cwd: Option<String> = None;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(row) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match row.get("type").and_then(|t| t.as_str()) {
            Some("session") => {
                if let Some(cwd) = row.get("cwd").and_then(|v| v.as_str()) {
                    session_cwd = Some(cwd.to_string());
                }
            }
            Some("message") => {
                let Some(msg) = row.get("message") else { continue };
                let uuid = row
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                if uuid.is_empty() {
                    continue;
                }
                let parent = row.get("parentId").and_then(|v| v.as_str());
                // 外层 timestamp（ISO）优先；toolResult 行 message 内还有
                // epoch ms 的 timestamp，作兜底转换
                let timestamp = row
                    .get("timestamp")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .or_else(|| {
                        msg.get("timestamp")
                            .and_then(|v| v.as_i64())
                            .filter(|ms| *ms > 0)
                            .and_then(epoch_ms_to_rfc3339)
                    })
                    .unwrap_or_default();

                let content = msg.get("content").cloned().unwrap_or(Value::Null);
                match msg.get("role").and_then(|r| r.as_str()) {
                    Some("user") => {
                        let mut m = pi_msg(uuid, parent, MessageType::User, "user", timestamp, &session_cwd);
                        m.tool_calls = extract_tool_calls(&content);
                        m.tool_results = extract_tool_results(&content);
                        m.content = content;
                        out.push(m);
                    }
                    Some("assistant") => {
                        let converted = convert_assistant_blocks(&content);
                        let mut m =
                            pi_msg(uuid, parent, MessageType::Assistant, "assistant", timestamp, &session_cwd);
                        m.tool_calls = extract_tool_calls(&converted);
                        m.content = converted;
                        m.usage = msg.get("usage").and_then(convert_usage);
                        m.model = msg.get("model").and_then(|v| v.as_str()).map(String::from);
                        out.push(m);
                    }
                    Some("toolResult") => {
                        // → Claude 协议：user 消息内 tool_result 块（is_meta 折叠）
                        let block = serde_json::json!({
                            "type": "tool_result",
                            "tool_use_id": msg.get("toolCallId").and_then(|v| v.as_str()).unwrap_or(""),
                            "content": msg.get("content").cloned().unwrap_or(Value::Null),
                            "is_error": msg.get("isError").and_then(|v| v.as_bool()).unwrap_or(false),
                        });
                        let wrapped = Value::Array(vec![block]);
                        let mut m = pi_msg(uuid, parent, MessageType::User, "user", timestamp, &session_cwd);
                        m.is_meta = true;
                        m.tool_results = extract_tool_results(&wrapped);
                        m.content = wrapped;
                        out.push(m);
                    }
                    _ => {}
                }
            }
            // 已知噪声行与未知行：静默跳过（与 claude serde 容错同语义）
            _ => {}
        }
    }
    out
}

/// epoch ms → RFC3339（UTC；与 claude 时间戳格式对齐，
/// `calculate_metrics`/`parse_ts_ms` 均按 rfc3339 解析）。
fn epoch_ms_to_rfc3339(ms: i64) -> Option<String> {
    chrono::DateTime::from_timestamp_millis(ms).map(|dt| dt.to_rfc3339())
}

/// 读首行 session 头。返回 `(cwd, created_ms)`；type 不符 / 无 cwd /
/// 读取失败 → None（调用方跳过该文件，不污染同目录其他文件）。
fn read_session_head(path: &Path, fs: &dyn FsProvider) -> Option<(String, u64)> {
    let head = fs.read_file_head(path, 1).ok()?;
    let first = head.lines().next()?;
    let row: Value = serde_json::from_str(first).ok()?;
    if row.get("type").and_then(|t| t.as_str()) != Some("session") {
        return None;
    }
    let cwd = row
        .get("cwd")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())?
        .to_string();
    let created_ms = row
        .get("timestamp")
        .and_then(|v| v.as_str())
        .and_then(crate::utils::timestamp::parse_ts_ms_opt)
        .unwrap_or(0) as u64;
    Some((cwd, created_ms))
}

/// 头部 light preview：首条用户消息 + 消息计数（200 行截断，对齐 claude
/// `extract_session_preview` 语义）。
///
/// 计数口径与 claude light 一致：`is_user_chunk_message` = type==user 且
/// !sidechain —— **含 tool result 载体行**（映射后 user+isMeta）。即每个
/// user/toolResult 行 +1、其后首个 assistant +1（agentic 轮内交替也计）。
struct PiPreview {
    first_message: Option<String>,
    message_count: u32,
}

fn extract_preview(path: &Path, fs: &dyn FsProvider) -> PiPreview {
    let head = fs.read_file_head(path, 200).unwrap_or_default();
    let mut preview = PiPreview {
        first_message: None,
        message_count: 0,
    };
    let mut awaiting_ai = false;
    for line in head.lines() {
        let Ok(row) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if row.get("type").and_then(|t| t.as_str()) != Some("message") {
            continue;
        }
        let Some(msg) = row.get("message") else { continue };
        match msg.get("role").and_then(|r| r.as_str()) {
            Some("user") | Some("toolResult") => {
                preview.message_count += 1;
                awaiting_ai = true;
                if preview.first_message.is_none() {
                    let text = msg
                        .get("content")
                        .and_then(|c| c.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|b| {
                                    (b.get("type").and_then(|t| t.as_str()) == Some("text"))
                                        .then(|| b.get("text").and_then(|v| v.as_str()))
                                        .flatten()
                                })
                                .collect::<Vec<_>>()
                                .join(" ")
                        })
                        .unwrap_or_default();
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        preview.first_message = Some(trimmed.chars().take(100).collect());
                    }
                }
            }
            Some("assistant") if awaiting_ai => {
                preview.message_count += 1;
                awaiting_ai = false;
            }
            _ => {}
        }
    }
    preview
}

impl AgentAdapter for PiAdapter {
    fn kind(&self) -> AgentKind {
        AgentKind::Pi
    }

    fn data_roots(&self) -> Vec<PathBuf> {
        vec![dirs::home_dir()
            .unwrap_or_default()
            .join(".pi")
            .join("agent")
            .join("sessions")]
    }

    fn owns_path(&self, path: &Path) -> bool {
        // 结构特征：组件序列含 `.pi/agent/sessions`（本地与 SSH 远端路径通用）
        path_has_components(path, &[".pi", "agent", "sessions"])
    }

    fn data_root_under(&self, home: &Path) -> PathBuf {
        home.join(".pi").join("agent").join("sessions")
    }

    fn scan_sessions(&self, root: &Path, fs: &dyn FsProvider) -> Vec<AgentSessionEntry> {
        let mut entries = Vec::new();
        let Ok(project_dirs) = fs.read_dir(root) else {
            return entries;
        };
        for dir in &project_dirs {
            if !dir.is_directory {
                continue;
            }
            let dir_path = root.join(&dir.name);
            let Ok(files) = fs.read_dir(&dir_path) else {
                continue;
            };
            for f in &files {
                if !f.is_file || !f.name.ends_with(".jsonl") {
                    continue;
                }
                let file_path = dir_path.join(&f.name);
                // 每个文件读自己的首行：半写/损坏文件只影响它自己，
                // 不会用坏头污染同目录其他会话（曾因目录级缓存引入此缺陷）
                let Some((cwd, created_ms)) = read_session_head(&file_path, fs) else {
                    log::debug!("pi: skip file without valid session head: {}", file_path.display());
                    continue;
                };
                let stem = f.name.trim_end_matches(".jsonl");
                entries.push(AgentSessionEntry {
                    agent: AgentKind::Pi,
                    project_id: encode_path(&cwd),
                    project_path: cwd,
                    session_id: session_id_from_stem(stem),
                    file_path,
                    mtime_ms: f.mtime_ms.unwrap_or(0),
                    birthtime_ms: f.birthtime_ms.unwrap_or(0),
                    created_ms,
                });
            }
        }
        entries
    }

    fn locate_session(
        &self,
        root: &Path,
        session_id: &str,
        fs: &dyn FsProvider,
        id_matches: &dyn Fn(&str) -> bool,
    ) -> Option<PathBuf> {
        // pi 目录编码与统一项目 id 不同构，无法从 project_id 直推目录名 →
        // 枚举项目目录找 `*_{session_id}.jsonl`；候选必须过项目归属校验
        // （读其 cwd），防止跨项目/陈旧 project_id 误命中
        let project_dirs = fs.read_dir(root).ok()?;
        for dir in &project_dirs {
            if !dir.is_directory {
                continue;
            }
            let dir_path = root.join(&dir.name);
            let Ok(files) = fs.read_dir(&dir_path) else {
                continue;
            };
            for f in &files {
                if !f.is_file || !f.name.ends_with(".jsonl") {
                    continue;
                }
                let stem = f.name.trim_end_matches(".jsonl");
                if session_id_from_stem(stem) != session_id {
                    continue;
                }
                let candidate = dir_path.join(&f.name);
                match read_session_head(&candidate, fs) {
                    Some((cwd, _)) if id_matches(&cwd) => return Some(candidate),
                    // 同 id 多副本（续期写出的第二个文件）且归属不符，或头
                    // 损坏：继续找下一份，不抢占
                    _ => continue,
                }
            }
        }
        None
    }

    fn parse_messages(&self, path: &Path, fs: &dyn FsProvider) -> Vec<ParsedMessage> {
        let content = match fs.read_file(path) {
            Ok(c) => c,
            Err(_) => return vec![],
        };
        parse_pi_content(&content)
    }

    fn light_session(&self, entry: &AgentSessionEntry, fs: &dyn FsProvider) -> Option<Session> {
        // entry 携带 scan 阶段读取的权威 cwd/created（不重读文件头，
        // 消除 scan/light 双读不自洽）
        let preview = extract_preview(&entry.file_path, fs);
        let created_at = if entry.created_ms > 0 {
            entry.created_ms
        } else if entry.birthtime_ms > 0 {
            entry.birthtime_ms
        } else {
            entry.mtime_ms
        };
        Some(Session {
            id: entry.session_id.clone(),
            agent: AgentKind::Pi,
            project_id: entry.project_id.clone(),
            project_path: entry.project_path.clone(),
            created_at,
            updated_at: Some(entry.mtime_ms),
            todo_data: None,
            first_message: preview.first_message,
            message_timestamp: None,
            has_subagents: false,
            message_count: preview.message_count,
            is_ongoing: None,
            git_branch: None,
            metadata_level: Some(SessionMetadataLevel::Light),
            context_consumption: None,
            compaction_count: None,
            phase_breakdown: None,
        })
    }

    fn resolve_watch_event(
        &self,
        path: &Path,
        fs: &dyn FsProvider,
    ) -> Option<(String, String)> {
        let name = path.file_name()?.to_str()?;
        if !name.ends_with(".jsonl") {
            return None;
        }
        let stem = name.trim_end_matches(".jsonl");
        // 只处理会话文件命名（{ts}_{uuid}.jsonl）；其他 jsonl（边车）不产事件
        let (cwd, _) = read_session_head(path, fs)?;
        Some((session_id_from_stem(stem), cwd))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::fs_provider::LocalFsProvider;
    use std::sync::Arc;

    const SAMPLE: &str = concat!(
        r#"{"type":"session","version":3,"id":"01a03770-891a-7792-a043-5f1dc085467b","timestamp":"2026-08-25T05:41:57.146Z","cwd":"/Users/x/proj","tools":[]}"#,
        "\n",
        r#"{"type":"message","id":"m001","parentId":null,"timestamp":"2026-08-25T05:42:00.000Z","message":{"role":"user","content":[{"type":"text","text":"hello pi"}]}}"#,
        "\n",
        r#"{"type":"message","id":"m002","parentId":"m001","timestamp":"2026-08-25T05:42:05.000Z","message":{"role":"assistant","model":"glm-5.3","usage":{"input":100,"output":50,"cacheRead":10,"cacheWrite":5,"reasoning":7,"totalTokens":172},"content":[{"type":"thinking","thinking":"let me think","thinkingSignature":"sig"},{"type":"text","text":"reading file"},{"type":"toolCall","id":"call_1","name":"read","arguments":{"path":"/tmp/a.rs"}}]}}"#,
        "\n",
        r#"{"type":"message","id":"m003","parentId":"m002","timestamp":"2026-08-25T05:42:06.000Z","message":{"role":"toolResult","toolCallId":"call_1","toolName":"read","content":[{"type":"text","text":"file body"}],"isError":false,"timestamp":1787636526000}}"#,
        "\n",
        r#"{"type":"model_change","id":"c1","timestamp":"2026-08-25T05:41:57.408Z","provider":"p","modelId":"glm-5.3"}"#,
        "\n",
        r#"{"type":"message","id":"m004","parentId":"m003","timestamp":"2026-08-25T05:42:10.000Z","message":{"role":"assistant","model":"glm-5.3","usage":{"input":80,"output":20},"content":[{"type":"text","text":"done"}]}}"#,
        "\n",
    );

    fn write_sample(dir: &tempfile::TempDir) -> std::path::PathBuf {
        let root = dir.path().join(".pi").join("agent").join("sessions");
        let proj = root.join("--Users-x-proj--");
        std::fs::create_dir_all(&proj).unwrap();
        let path = proj.join("2026-08-25T05-41-57-146Z_01a03770-891a-7792-a043-5f1dc085467b.jsonl");
        std::fs::write(&path, SAMPLE).unwrap();
        path
    }

    #[test]
    fn parse_maps_pi_to_claude_semantics() {
        let msgs = parse_pi_content(SAMPLE);
        assert_eq!(msgs.len(), 4);

        // user 原样（text 块同构）
        assert_eq!(msgs[0].uuid, "m001");
        assert_eq!(msgs[0].message_type, MessageType::User);
        assert!(!msgs[0].is_meta);
        assert_eq!(msgs[0].cwd.as_deref(), Some("/Users/x/proj"));

        // assistant：thinking/toolCall 块已转换，usage/model 已映射
        let a = &msgs[1];
        assert_eq!(a.message_type, MessageType::Assistant);
        assert_eq!(a.model.as_deref(), Some("glm-5.3"));
        let usage = a.usage.as_ref().unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.cache_read_input_tokens, Some(10));
        assert_eq!(usage.cache_creation_input_tokens, Some(5));
        assert_eq!(a.tool_calls.len(), 1);
        assert_eq!(a.tool_calls[0].name, "read");
        assert_eq!(
            a.tool_calls[0].input,
            serde_json::json!({"path": "/tmp/a.rs"})
        );
        // content 中 tool_use / thinking 块已按 Claude 协议转换
        let blocks = a.content.as_array().unwrap();
        assert_eq!(blocks[0].get("type").unwrap(), "thinking");
        assert_eq!(blocks[0].get("signature").unwrap(), "sig");
        assert_eq!(blocks[2].get("type").unwrap(), "tool_use");

        // toolResult → user + tool_result 块 + is_meta
        let tr = &msgs[2];
        assert_eq!(tr.message_type, MessageType::User);
        assert!(tr.is_meta);
        assert_eq!(tr.tool_results.len(), 1);
        assert_eq!(tr.tool_results[0].tool_use_id, "call_1");
        assert!(!tr.tool_results[0].is_error);

        // 噪声行 model_change 不产出消息
        assert_eq!(msgs[3].uuid, "m004");
    }

    #[test]
    fn parsed_output_survives_downstream_pipeline() {
        // 契约测试：pi 映射结果能穿过 claude 下游管线（分类 + chunk 构建）
        let msgs = parse_pi_content(SAMPLE);
        let session = crate::parsing::process_messages(&msgs);
        assert!(session.metrics.input_tokens > 0);
        let chunks = crate::analysis::ChunkBuilder::build_chunks(&msgs, &[]);
        assert!(!chunks.is_empty());
    }

    #[test]
    fn scan_and_locate_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_sample(&dir);
        let fs = Arc::new(LocalFsProvider::new());
        let adapter = PiAdapter::new();
        // scan/locate 的 root 语义是 sessions 根（项目目录的父目录），
        // 与 data_root_under(home) 的产出同层
        let root = path.parent().unwrap().parent().unwrap().to_path_buf();

        let entries = adapter.scan_sessions(&root, fs.as_ref());
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.project_id, "-Users-x-proj");
        assert_eq!(e.project_path, "/Users/x/proj");
        assert_eq!(e.session_id, "01a03770-891a-7792-a043-5f1dc085467b");
        assert!(e.created_ms > 0, "首行 timestamp 已转为 epoch ms");

        // locate：归属校验通过才返回
        let ok = |cwd: &str| cwd == "/Users/x/proj";
        let found = adapter.locate_session(&root, "01a03770-891a-7792-a043-5f1dc085467b", fs.as_ref(), &ok);
        assert_eq!(found.as_ref(), Some(&path));
        // 归属不符（别的项目）→ 不返回
        let reject = |_: &str| false;
        assert!(adapter
            .locate_session(&root, "01a03770-891a-7792-a043-5f1dc085467b", fs.as_ref(), &reject)
            .is_none());

        // light session 构造：单次 preview 读，计数口径 = user+配对 assistant
        let light = adapter.light_session(e, fs.as_ref()).unwrap();
        assert_eq!(light.agent, AgentKind::Pi);
        assert_eq!(light.first_message.as_deref(), Some("hello pi"));
        // 4 = user(m001) + assistant(m002) + toolResult(m003 载体行) +
        // assistant(m004)：严格对齐 claude 口径（toolResult 后的 assistant
        // 二次配对也计）
        assert_eq!(light.message_count, 4);
        assert_eq!(light.project_path, "/Users/x/proj");
    }

    /// finding 3 回归：同目录首个文件头损坏（半写），不得污染其余会话。
    #[test]
    fn scan_skips_bad_head_file_without_polluting_siblings() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("sessions");
        let proj = root.join("--Users-x-proj--");
        std::fs::create_dir_all(&proj).unwrap();
        // 坏文件：空内容（首行读不到 session 头）
        std::fs::write(proj.join("2026-08-25T00-00-00-000Z_badbadbad-0000-4000-8000-000000000001.jsonl"), "").unwrap();
        // 好文件
        std::fs::write(
            proj.join("2026-08-25T00-00-00-000Z_goodgood-0000-4000-8000-000000000002.jsonl"),
            "{\"type\":\"session\",\"cwd\":\"/Users/x/proj\",\"timestamp\":\"2026-08-25T00:00:00.000Z\"}\n",
        )
        .unwrap();
        let fs = Arc::new(LocalFsProvider::new());
        let entries = PiAdapter::new().scan_sessions(&root, fs.as_ref());
        assert_eq!(entries.len(), 1, "坏头文件只跳过自己，兄弟会话保留");
        assert_eq!(entries[0].session_id, "goodgood-0000-4000-8000-000000000002");
    }

    /// finding 10 回归：同 session id 出现在两个项目目录，locate 按归属校验
    /// 返回正确项目的文件（不依赖 read_dir 顺序）。
    #[test]
    fn locate_respects_project_scope() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("sessions");
        let proj_a = root.join("--a--");
        let proj_b = root.join("--b--");
        std::fs::create_dir_all(&proj_a).unwrap();
        std::fs::create_dir_all(&proj_b).unwrap();
        let sid = "aaaabbbb-0000-4000-8000-000000000000";
        let file_a = proj_a.join(format!("2026-08-25T00-00-00-000Z_{sid}.jsonl"));
        let file_b = proj_b.join(format!("2026-08-26T00-00-00-000Z_{sid}.jsonl"));
        std::fs::write(&file_a, "{\"type\":\"session\",\"cwd\":\"/a\"}\n").unwrap();
        std::fs::write(&file_b, "{\"type\":\"session\",\"cwd\":\"/b\"}\n").unwrap();

        let fs = Arc::new(LocalFsProvider::new());
        let adapter = PiAdapter::new();
        // 请求项目 /b：无论 read_dir 顺序，必须返回 file_b
        let wants_b = |cwd: &str| cwd == "/b";
        let found = adapter.locate_session(&root, sid, fs.as_ref(), &wants_b);
        assert_eq!(found.as_ref(), Some(&file_b));
    }

    #[test]
    fn owns_path_matches_structure() {
        assert!(PiAdapter::new().owns_path(Path::new(
            "/home/u/.pi/agent/sessions/--x--/2026-08-25T05-41-57-146Z_abc.jsonl"
        )));
        assert!(!PiAdapter::new().owns_path(Path::new(
            "/home/u/.claude/projects/-x-/abc.jsonl"
        )));
    }

    /// 真实数据 smoke（本机装有 pi 时）：扫描 → 解析 → 下游管线全链。
    /// 手动触发：`cargo test -- --ignored`
    #[test]
    #[ignore = "依赖本机 ~/.pi 真实数据"]
    fn real_data_smoke() {
        let home = dirs::home_dir().unwrap();
        let root = PiAdapter::new().data_root_under(&home);
        if !root.is_dir() {
            eprintln!("no real pi data, skip");
            return;
        }
        let fs = Arc::new(LocalFsProvider::new());
        let adapter = PiAdapter::new();
        let entries = adapter.scan_sessions(&root, fs.as_ref());
        eprintln!("scanned {} pi sessions", entries.len());
        assert!(!entries.is_empty(), "本机应有 pi 会话数据");

        // 每个会话：light 元数据 + 全量解析 + 下游 chunk 构建不 panic
        for e in &entries {
            let light = adapter
                .light_session(e, fs.as_ref())
                .unwrap_or_else(|| panic!("light_session failed for {:?}", e.file_path));
            assert_eq!(light.agent, AgentKind::Pi);
            let msgs = adapter.parse_messages(&e.file_path, fs.as_ref());
            assert!(!msgs.is_empty(), "no messages parsed from {:?}", e.file_path);
            let parsed = crate::parsing::process_messages(&msgs);
            let chunks = crate::analysis::ChunkBuilder::build_chunks(&msgs, &[]);
            eprintln!(
                "  {} → {} msgs, {} chunks, {} tokens",
                e.session_id,
                msgs.len(),
                chunks.len(),
                parsed.metrics.total_tokens
            );
        }

        // locate 全部命中（归属校验 = 自编码 id）
        for e in &entries {
            let pid = e.project_id.clone();
            let ok = move |cwd: &str| encode_path(cwd) == pid;
            let found = adapter.locate_session(&root, &e.session_id, fs.as_ref(), &ok);
            assert_eq!(found.as_deref(), Some(e.file_path.as_path()));
        }
    }
}
