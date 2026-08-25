//! Codex CLI 适配器。
//!
//! 数据源：`~/.codex/sessions/YYYY/MM/DD/rollout-{ts}-{sessionId}.jsonl`（按日期
//! 三层组织，无项目维度）+ `~/.codex/archived_sessions/`（第二根，实测平铺）。
//! 归属项目由首行 `session_meta.payload.cwd` 给出（权威，与 pi 同策略）。
//! `CODEX_HOME` 环境变量支持：仅对本地上下文生效（远端 SSH home 无从感知
//! 本进程 env，采信反而读错树）。
//!
//! # rollout 格式要点（2025-09 ~ 2026-03 本机实测）
//!
//! 三种信封，均带顶层 `timestamp`：
//! - `session_meta`：`payload.{cwd, git.branch, id, originator}`（首行，权威）；
//! - `turn_context`：`payload.{cwd, model}` —— cwd 兜底、model 记录；
//! - `response_item`（**主路径**，OpenAI Response API 形态）：
//!   - `message`：`{role, content:[{type:input_text|output_text, text}]}`，
//!     无 id 字段（uuid 以行序合成）；
//!   - `reasoning`：`{summary:[{type:summary_text,text}], encrypted_content}`
//!     —— 只取明文 summary 作 thinking（encrypted 丢弃）；
//!   - `function_call`：`{name, call_id, arguments:JSON字符串}`；
//!   - `custom_tool_call` / `local_shell_call`：同构（input/action 变体）；
//!   - `*_output`：`{call_id, output}`，output 为字符串化 JSON 或对象；
//! - `event_msg`（**降级路径**，无 response_item 的老会话）：
//!   `user_message`/`agent_message`（正文）、`token_count`
//!   （`info.total_token_usage` 为**累计值**，按相邻差分得单轮增量）；
//! - `compacted`：上下文压缩标记 → `is_compact_summary`。
//!
//! # 与 Claude 管线的能力差异（如实声明）
//!
//! - 无 per-message usage：token_count 差分挂最近 assistant 消息（一个 turn
//!   多次 token_count 时相加），sum(metrics) ≈ 末次累计值；
//! - 无 todos / CLAUDE.md 生态；子代理（compact 分身）本机数据未观测到，
//!   `agent_type` 相关字段暂不解析；
//! - `state_5.sqlite`（threads 表：手动命名标题）暂未接入 —— 标题按首条
//!   真人消息推导。

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::agents::{path_has_components, AgentAdapter, AgentSessionEntry};
use crate::infrastructure::fs_provider::{FsDirent, FsProvider};
use crate::types::domain::{AgentKind, MessageType, Session, SessionMetadataLevel};
use crate::types::jsonl::UsageMetadata;
use crate::types::messages::ParsedMessage;
use crate::utils::encode_path;

pub struct CodexAdapter {
    /// CODEX_HOME 采信结果（目录下确有 sessions/archived_sessions 才采信，
    /// 对齐 Wake 语义：空目录不采信，否则整家会话凭空消失）。
    custom_base: Option<PathBuf>,
}

impl CodexAdapter {
    pub fn new() -> Self {
        let custom_base = std::env::var_os("CODEX_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .filter(|p| p.join("sessions").is_dir() || p.join("archived_sessions").is_dir());
        Self { custom_base }
    }
}

impl Default for CodexAdapter {
    fn default() -> Self {
        Self::new()
    }
}

/// `rollout-{ts}-{sessionId}` stem → sessionId（剥 19 字符时间戳前缀 +
/// 分隔符；不匹配则原样兜底）。
fn session_id_from_stem(stem: &str) -> String {
    if let Some(rest) = stem.strip_prefix("rollout-") {
        if rest.len() > 20 && rest.as_bytes()[10] == b'T' {
            return rest[20..].to_string();
        }
    }
    stem.to_string()
}

/// IDE/CLI 注入型「用户」消息前缀（实测 + Wake `is_injected_user_content`
/// 清单裁剪）：非真人手打，归 `is_meta`。
fn is_injected_user_text(text: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "<environment_context",
        "<user_instructions",
        "<permissions",
        "<session_context",
        "<system-",
        "<context ",
        "# AGENTS.md instructions",
        "# Context from my IDE setup",
        "IMPORTANT: Do NOT read",
        "Caveat: The messages below",
    ];
    let t = text.trim_start();
    PREFIXES.iter().any(|p| t.starts_with(p)) || t.contains("/.codex/plugins/")
}

/// 合成 uuid：rollout 行无消息级 id，按行序合成（同文件行序稳定，
/// 全量重解析下 chunk id `{type}-{uuid}` 稳定）。
fn synth_uuid(seq: usize) -> String {
    format!("codex-{seq}")
}

/// rollout 文本提取：message.content blocks 的 input_text/output_text/text。
fn message_text(payload: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    match payload.get("content") {
        Some(Value::Array(blocks)) => {
            for b in blocks {
                if matches!(
                    b.get("type").and_then(|t| t.as_str()),
                    Some("input_text") | Some("output_text") | Some("text")
                ) {
                    if let Some(t) = b.get("text").and_then(|v| v.as_str()) {
                        parts.push(t.to_string());
                    }
                }
            }
        }
        Some(Value::String(s)) => parts.push(s.clone()),
        _ => {}
    }
    parts.join("\n\n").trim().to_string()
}

/// tool_call payload → `{name, call_id, arguments/input/action}` 归一。
struct RawCall {
    call_id: String,
    name: String,
    input: Value,
}

fn raw_call(payload: &Value) -> RawCall {
    let call_id = payload
        .get("call_id")
        .or_else(|| payload.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let name = payload
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("tool")
        .to_string();
    // function_call.arguments 是 JSON 字符串；custom/local 变体在 input/action
    let raw = payload
        .get("arguments")
        .and_then(|v| v.as_str())
        .or_else(|| payload.get("input").and_then(|v| v.as_str()))
        .map(String::from);
    let input = match raw {
        Some(s) if !s.is_empty() => {
            serde_json::from_str::<Value>(&s).unwrap_or(Value::String(s))
        }
        _ => payload
            .get("action")
            .cloned()
            .unwrap_or(Value::Null),
    };
    RawCall { call_id, name, input }
}

/// *_call_output payload → 输出文本（字符串化 JSON 内层 output / 对象 content
/// / 原字符串）。
fn output_text(payload: &Value) -> String {
    match payload.get("output") {
        Some(Value::String(s)) => {
            // 字符串化 JSON：尝试剥内层 {"output": ...} 壳（实测形态）
            match serde_json::from_str::<Value>(s) {
                Ok(Value::Object(o)) => o
                    .get("output")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| s.clone()),
                _ => s.clone(),
            }
        }
        Some(o @ Value::Object(_)) => o
            .get("content")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| serde_json::to_string(o).unwrap_or_default()),
        _ => String::new(),
    }
}

/// 解析上下文（单文件全量解析的共享状态）。
struct CodexParse {
    messages: Vec<ParsedMessage>,
    cwd: Option<String>,
    git_branch: Option<String>,
    model: Option<String>,
    last_total_tokens: Option<u64>,
    created_ms: u64,
    last_ts: u64,
}

/// token_count 累计值差分 → 单轮增量，挂到最近一条 assistant（已有 usage
/// 则相加：一个 turn 多次 token_count = 重试场景）。
fn attach_usage(p: &mut CodexParse, total: u64) {
    let Some(last) = p.messages.last_mut() else { return };
    if last.message_type != MessageType::Assistant {
        return;
    }
    let delta = total - p.last_total_tokens.unwrap_or(0);
    let usage = last.usage.take().unwrap_or(UsageMetadata {
        input_tokens: 0,
        output_tokens: 0,
        cache_read_input_tokens: None,
        cache_creation_input_tokens: None,
    });
    last.usage = Some(UsageMetadata {
        input_tokens: usage.input_tokens + delta,
        output_tokens: usage.output_tokens,
        cache_read_input_tokens: usage
            .cache_read_input_tokens
            .or(Some(0)),
        cache_creation_input_tokens: usage.cache_creation_input_tokens,
    });
}

fn codex_msg(seq: &mut usize, ty: MessageType, role: &str, ts: u64, cwd: &Option<String>) -> ParsedMessage {
    *seq += 1;
    ParsedMessage {
        uuid: synth_uuid(*seq),
        parent_uuid: None,
        message_type: ty,
        timestamp: chrono::DateTime::from_timestamp_millis(ts as i64)
            .map(|d| d.to_rfc3339())
            .unwrap_or_default(),
        role: Some(role.to_string()),
        content: Value::Null,
        usage: None,
        model: None,
        cwd: cwd.clone(),
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

/// rollout JSONL 内容 → ParsedMessage（模块文档映射表）。
pub(crate) fn parse_codex_content(content: &str) -> CodexParse {
    let mut p = CodexParse {
        messages: Vec::new(),
        cwd: None,
        git_branch: None,
        model: None,
        last_total_tokens: None,
        created_ms: 0,
        last_ts: 0,
    };
    let mut seq = 0usize;
    let mut saw_session_meta = false;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(row) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let ts = row
            .get("timestamp")
            .and_then(|v| v.as_str())
 .and_then(crate::utils::timestamp::parse_ts_ms_opt)
            .unwrap_or(0);
        if ts > 0 {
            if p.created_ms == 0 {
                p.created_ms = ts as u64;
            }
            p.last_ts = p.last_ts.max(ts as u64);
        }
        let typ = row.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let Some(payload) = row.get("payload") else {
            // 无 payload 的顶层类型：compacted 单独处理，其余跳过
            if typ == "compacted" {
                let mut m = codex_msg(&mut seq, MessageType::User, "user", ts, &p.cwd);
                m.is_meta = true;
                m.is_compact_summary = Some(true);
                m.content = Value::String("── Context compacted ──".to_string());
                p.messages.push(m);
            }
            continue;
        };

        match typ {
            "session_meta" => {
                if !saw_session_meta {
                    saw_session_meta = true;
                    if let Some(c) = payload.get("cwd").and_then(|v| v.as_str()) {
                        p.cwd = Some(c.to_string());
                    }
                    if let Some(b) = payload
                        .pointer("/git/branch")
                        .and_then(|v| v.as_str())
                    {
                        p.git_branch = Some(b.to_string());
                    }
                }
            }
            "turn_context" => {
                if p.cwd.is_none() {
                    if let Some(c) = payload.get("cwd").and_then(|v| v.as_str()) {
                        p.cwd = Some(c.to_string());
                    }
                }
                if let Some(m) = payload.get("model").and_then(|v| v.as_str()) {
                    p.model = Some(m.to_string());
                }
            }
            "response_item" => match payload.get("type").and_then(|t| t.as_str()) {
                Some("message") => {
                    let text = message_text(payload);
                    if text.is_empty() {
                        continue;
                    }
                    match payload.get("role").and_then(|r| r.as_str()) {
                        Some("user") => {
                            let mut m = codex_msg(&mut seq, MessageType::User, "user", ts, &p.cwd);
                            m.is_meta = is_injected_user_text(&text);
                            m.content = Value::String(text);
                            p.messages.push(m);
                        }
                        Some("assistant") => {
                            let mut m =
                                codex_msg(&mut seq, MessageType::Assistant, "assistant", ts, &p.cwd);
                            m.content =
                                Value::Array(vec![serde_json::json!({"type": "text", "text": text})]);
                            m.model = p.model.clone();
                            p.messages.push(m);
                        }
                        _ => {
                            let mut m = codex_msg(&mut seq, MessageType::System, "system", ts, &p.cwd);
                            m.is_meta = true;
                            m.content = Value::String(text);
                            p.messages.push(m);
                        }
                    }
                }
                Some("reasoning") => {
                    // 明文 summary → thinking 块；附着到前一条空 assistant 或新建
                    let thinking: String = payload
                        .get("summary")
                        .and_then(|s| s.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|s| s.get("text").and_then(|v| v.as_str()))
                                .collect::<Vec<_>>()
                                .join("\n\n")
                        })
                        .unwrap_or_default();
                    if thinking.is_empty() {
                        continue;
                    }
                    // 附着判定：前一条是 assistant 且尚无 thinking 块且无工具
                    let attach = match p.messages.last() {
                        Some(m) if m.message_type == MessageType::Assistant => {
                            let has_thinking = m
                                .content
                                .as_array()
                                .is_some_and(|arr| {
                                    arr.iter().any(|b| b.get("type") == Some(&serde_json::json!("thinking")))
                                });
                            !has_thinking && m.tool_calls.is_empty()
                        }
                        _ => false,
                    };
                    if attach {
                        if let Some(last) = p.messages.last_mut() {
                            last.set_thinking(&thinking);
                        }
                    } else {
                        let mut m =
                            codex_msg(&mut seq, MessageType::Assistant, "assistant", ts, &p.cwd);
                        m.content = Value::Array(vec![serde_json::json!({
                            "type": "thinking", "thinking": thinking, "signature": ""
                        })]);
                        m.model = p.model.clone();
                        p.messages.push(m);
                    }
                }
                Some("function_call") | Some("custom_tool_call") | Some("local_shell_call") => {
                    let call = raw_call(payload);
                    // tool_use 附着到最近 assistant（无则新建宿主消息）
                    let host_exists = matches!(
                        p.messages.last(),
                        Some(m) if m.message_type == MessageType::Assistant
                    );
                    if !host_exists {
                        let m = codex_msg(&mut seq, MessageType::Assistant, "assistant", ts, &p.cwd);
                        p.messages.push(m);
                    }
                    let block = serde_json::json!({
                        "type": "tool_use",
                        "id": call.call_id,
                        "name": call.name,
                        "input": call.input,
                    });
                    if let Some(last) = p.messages.last_mut() {
                        if let Some(arr) = last.content.as_array_mut() {
                            arr.push(block);
                        } else {
                            last.content = Value::Array(vec![block]);
                        }
                        last.tool_calls.push(crate::types::messages::ToolCall {
                            id: call.call_id,
                            name: call.name,
                            input: call.input,
                            is_task: false,
                            task_description: None,
                            task_subagent_type: None,
                        });
                        if last.model.is_none() {
                            last.model = p.model.clone();
                        }
                    }
                }
                Some("function_call_output") | Some("custom_tool_call_output") => {
                    let call_id = payload
                        .get("call_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let text = output_text(payload);
                    let is_error = payload.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);
                    let block = serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": call_id,
                        "content": text,
                        "is_error": is_error,
                    });
                    let mut m = codex_msg(&mut seq, MessageType::User, "user", ts, &p.cwd);
                    m.is_meta = true;
                    m.content = Value::Array(vec![block]);
                    m.tool_results = crate::parsing::extract_tool_results(&m.content);
                    p.messages.push(m);
                }
                // web_search_call 等其余 item：暂不展示
                _ => {}
            },
            "event_msg" => match payload.get("type").and_then(|t| t.as_str()) {
                Some("token_count") => {
                    if let Some(total) = payload
                        .pointer("/info/total_token_usage/total_tokens")
                        .and_then(|v| v.as_u64())
                    {
                        if p.last_total_tokens.map_or(true, |last| total >= last) {
                            attach_usage(&mut p, total);
                            p.last_total_tokens = Some(total);
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    p
}

/// ParsedMessage 辅助：thinking 块注入（reasoning 附着分支）。
trait SetThinking {
    fn set_thinking(&mut self, thinking: &str);
}

impl SetThinking for ParsedMessage {
    fn set_thinking(&mut self, thinking: &str) {
        let block = serde_json::json!({
            "type": "thinking", "thinking": thinking, "signature": ""
        });
        match &mut self.content {
            Value::Array(arr) => arr.insert(0, block),
            _ => self.content = Value::Array(vec![block]),
        }
    }
}

/// 读 rollout 首行 session_head。返回 `(cwd, git_branch, created_ms)`；
/// 非 rollout / 无 cwd → None。
fn read_rollout_head(path: &Path, fs: &dyn FsProvider) -> Option<(String, Option<String>, u64)> {
    let head = fs.read_file_head(path, 1).ok()?;
    let first = head.lines().next()?;
    let row: Value = serde_json::from_str(first).ok()?;
    if row.get("type").and_then(|t| t.as_str()) != Some("session_meta") {
        return None;
    }
    let cwd = row
        .pointer("/payload/cwd")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())?
        .to_string();
    let git_branch = row
        .pointer("/payload/git/branch")
        .and_then(|v| v.as_str())
        .map(String::from);
    let created_ms = row
        .get("timestamp")
        .and_then(|v| v.as_str())
        .and_then(crate::utils::timestamp::parse_ts_ms_opt)
        .unwrap_or(0) as u64;
    Some((cwd, git_branch, created_ms))
}

// 深度受限递归枚举 rollout 文件（sessions/YYYY/MM/DD 三层 + archived 平铺；
// 深 4 层覆盖两种布局，防异常深树）。返回 (完整路径, dirent 元数据) —— rollout
// 藏在日期子目录下，dirent 只有文件名，调用方无法从顶层根重建路径。
fn collect_rollout_files(
    dir: &Path,
    depth: usize,
    fs: &dyn FsProvider,
    out: &mut Vec<(PathBuf, FsDirent)>,
) {
    if depth > 4 {
        return;
    }
    let Ok(entries) = fs.read_dir(dir) else {
        return;
    };
    for e in entries {
        if e.is_file && e.name.starts_with("rollout-") && e.name.ends_with(".jsonl") {
            out.push((dir.join(&e.name), e));
        } else if e.is_directory {
            collect_rollout_files(&dir.join(&e.name), depth + 1, fs, out);
        }
    }
}

/// 头部 light preview：首条真人 user 消息 + 计数（user+配对 assistant）。
struct CodexPreview {
    first_message: Option<String>,
    message_count: u32,
}

fn extract_preview(content: &str) -> CodexPreview {
    let mut preview = CodexPreview {
        first_message: None,
        message_count: 0,
    };
    let mut awaiting_ai = false;
    let mut lines = content.lines();
    // 跳过 200 行截断与 pi 的 read_file_head 对齐：此处调用方已截断
    for _ in 0..200 {
        let Some(line) = lines.next() else { break };
        let Ok(row) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if row.get("type").and_then(|t| t.as_str()) != Some("response_item") {
            continue;
        }
        let Some(payload) = row.get("payload") else { continue };
        match payload.get("type").and_then(|t| t.as_str()) {
            // user 载体行（含注入）与 tool result 载体行都计数 —— 严格对齐
            // claude 的 is_user_chunk_message 口径（不排除 isMeta）
            Some("message") if payload.get("role").and_then(|r| r.as_str()) == Some("user") => {
                preview.message_count += 1;
                awaiting_ai = true;
                let text = message_text(payload);
                if !text.is_empty()
                    && preview.first_message.is_none()
                    && !is_injected_user_text(&text)
                {
                    preview.first_message = Some(text.chars().take(100).collect());
                }
            }
            Some("function_call_output") | Some("custom_tool_call_output") => {
                preview.message_count += 1;
                awaiting_ai = true;
            }
            Some("message") if awaiting_ai
                && payload.get("role").and_then(|r| r.as_str()) == Some("assistant") =>
            {
                preview.message_count += 1;
                awaiting_ai = false;
            }
            _ => {}
        }
    }
    preview
}

impl AgentAdapter for CodexAdapter {
    fn kind(&self) -> AgentKind {
        AgentKind::Codex
    }

    fn data_roots(&self) -> Vec<PathBuf> {
        let base = self
            .custom_base
            .clone()
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".codex"));
        vec![base.join("sessions"), base.join("archived_sessions")]
    }

    fn owns_path(&self, path: &Path) -> bool {
        path_has_components(path, &[".codex", "sessions"])
            || path_has_components(path, &[".codex", "archived_sessions"])
    }

    fn data_root_under(&self, home: &Path) -> PathBuf {
        // CODEX_HOME 只对本地上下文生效：远端 SSH home 无从感知本进程 env。
        // 返回 base（相当于 `.codex` 层），scan_sessions 内部扫两个子根
        if let Some(custom) = &self.custom_base {
            if Some(home) == dirs::home_dir().as_deref() {
                return custom.clone();
            }
        }
        home.join(".codex")
    }

    fn watch_roots_under(&self, home: &Path) -> Vec<PathBuf> {
        // 递归监听两个子根而非 .codex 整树：sqlite-wal/config/logs 的高频
        // 写入会以无效事件刷屏 debounce 队列
        let base = self.data_root_under(home);
        vec![base.join("sessions"), base.join("archived_sessions")]
    }

    fn scan_sessions(&self, root: &Path, fs: &dyn FsProvider) -> Vec<AgentSessionEntry> {
        // root = .../.codex；两个子根都扫
        let mut entries = Vec::new();
        for sub in ["sessions", "archived_sessions"] {
            let dir = root.join(sub);
            if !fs.exists(&dir).unwrap_or(false) {
                continue;
            }
            let mut files = Vec::new();
            collect_rollout_files(&dir, 0, fs, &mut files);
            for (file_path, f) in files {
                let Some((cwd, _git, created_ms)) = read_rollout_head(&file_path, fs) else {
                    continue;
                };
                let stem = f.name.trim_end_matches(".jsonl");
                entries.push(AgentSessionEntry {
                    agent: AgentKind::Codex,
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
        for sub in ["sessions", "archived_sessions"] {
            let dir = root.join(sub);
            if !fs.exists(&dir).unwrap_or(false) {
                continue;
            }
            let mut files = Vec::new();
            collect_rollout_files(&dir, 0, fs, &mut files);
            for (candidate, f) in files {
                let stem = f.name.trim_end_matches(".jsonl");
                if session_id_from_stem(stem) != session_id {
                    continue;
                }
                if let Some((cwd, _, _)) = read_rollout_head(&candidate, fs) {
                    if id_matches(&cwd) {
                        return Some(candidate);
                    }
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
        parse_codex_content(&content).messages
    }

    fn light_session(&self, entry: &AgentSessionEntry, fs: &dyn FsProvider) -> Option<Session> {
        let head = fs.read_file_head(&entry.file_path, 200).ok()?;
        let preview = extract_preview(&head);
        // git_branch 首行已带（scan 阶段读取），light 不重读
        let created_at = if entry.created_ms > 0 {
            entry.created_ms
        } else if entry.birthtime_ms > 0 {
            entry.birthtime_ms
        } else {
            entry.mtime_ms
        };
        Some(Session {
            id: entry.session_id.clone(),
            agent: AgentKind::Codex,
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
        if !(name.starts_with("rollout-") && name.ends_with(".jsonl")) {
            return None;
        }
        let stem = name.trim_end_matches(".jsonl");
        let (cwd, _, _) = read_rollout_head(path, fs)?;
        Some((session_id_from_stem(stem), cwd))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::AgentAdapter;
    use crate::infrastructure::fs_provider::LocalFsProvider;
    use std::sync::Arc;

    const SAMPLE: &str = concat!(
        r#"{"timestamp":"2026-08-25T05:00:00.000Z","type":"session_meta","payload":{"id":"s1","cwd":"/Users/x/proj","originator":"codex_cli_rs","git":{"branch":"main"}} }"#,
        "\n",
        r#"{"timestamp":"2026-08-25T05:00:00.100Z","type":"turn_context","payload":{"cwd":"/Users/x/proj","model":"gpt-5-codex"}}"#,
        "\n",
        r#"{"timestamp":"2026-08-25T05:00:01.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"what is 2+2?"}]}}"#,
        "\n",
        r#"{"timestamp":"2026-08-25T05:00:01.500Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>\n  <cwd>/</cwd>\n</environment_context>"}]}}"#,
        "\n",
        r#"{"timestamp":"2026-08-25T05:00:02.000Z","type":"response_item","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"thinking about it"}],"encrypted_content":"enc"}}"#,
        "\n",
        r#"{"timestamp":"2026-08-25T05:00:03.000Z","type":"response_item","payload":{"type":"function_call","name":"shell","call_id":"call_1","arguments":"{\"command\":[\"bash\",\"-lc\",\"ls\"]}"}}"#,
        "\n",
        r#"{"timestamp":"2026-08-25T05:00:04.000Z","type":"response_item","payload":{"type":"function_call_output","call_id":"call_1","output":"{\"output\":\"file1\\nfile2\"}"}}"#,
        "\n",
        r#"{"timestamp":"2026-08-25T05:00:05.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"It is 4."}]}}"#,
        "\n",
        r#"{"timestamp":"2026-08-25T05:00:06.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"output_tokens":50,"total_tokens":150}}}}"#,
        "\n",
        r#"{"timestamp":"2026-08-25T05:00:07.000Z","type":"compacted"}"#,
        "\n",
    );

    #[test]
    fn parse_maps_rollout_to_claude_semantics() {
        let p = parse_codex_content(SAMPLE);
        let msgs = p.messages;
        // 顺序：user(真人) / user(注入 meta) / assistant(thinking+tool_use 宿主) /
        // output→user(meta) / assistant(答案+usage) / compacted→user(meta, compact)
        assert_eq!(msgs.len(), 6);

        assert_eq!(msgs[0].message_type, MessageType::User);
        assert!(!msgs[0].is_meta, "真人输入");

        assert!(msgs[1].is_meta, "<environment_context 注入归 meta");
        assert!(is_injected_user_text("# AGENTS.md instructions for /x"));

        // reasoning 附着：前一条是 user（非空 assistant），故新建 thinking assistant
        assert_eq!(msgs[2].message_type, MessageType::Assistant);
        assert!(
            msgs[2].content.as_array().unwrap()[0].get("type") == Some(&serde_json::json!("thinking")),
            "reasoning → thinking 块"
        );

        // function_call 挂到 assistant 宿主（复用前一条 assistant？前一条是
        // thinking assistant → 复用），tool_use 块 + tool_calls
        let host = &msgs[2];
        assert_eq!(host.tool_calls.len(), 1);
        assert_eq!(host.tool_calls[0].name, "shell");
        assert_eq!(
            host.tool_calls[0].input,
            serde_json::json!({"command": ["bash", "-lc", "ls"]}),
            "arguments JSON 字符串解析为对象"
        );

        // output → user meta + tool_result
        assert_eq!(msgs[3].message_type, MessageType::User);
        assert!(msgs[3].is_meta);
        assert_eq!(msgs[3].tool_results[0].tool_use_id, "call_1");
        assert!(msgs[3].tool_results[0].content.as_str().unwrap().contains("file1"), "字符串化 JSON 剥内层壳");

        // 答案 assistant + token_count 差分 usage（150-0=150 挂最近 assistant）
        let answer = &msgs[4];
        assert_eq!(answer.message_type, MessageType::Assistant);
        assert_eq!(answer.model.as_deref(), Some("gpt-5-codex"), "turn_context model");
        assert_eq!(answer.usage.as_ref().unwrap().input_tokens, 150);

        // compacted
        assert!(msgs[5].is_compact_summary == Some(true));

        // 元信息
        assert_eq!(p.cwd.as_deref(), Some("/Users/x/proj"));
        assert_eq!(p.git_branch.as_deref(), Some("main"));
    }

    #[test]
    fn parsed_output_survives_downstream_pipeline() {
        let p = parse_codex_content(SAMPLE);
        let parsed = crate::parsing::process_messages(&p.messages);
        assert!(parsed.metrics.input_tokens > 0, "token_count 差分可累计");
        let chunks = crate::analysis::ChunkBuilder::build_chunks(&p.messages, &[]);
        assert!(!chunks.is_empty());
    }

    #[test]
    fn scan_locate_light_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        // sessions/YYYY/MM/DD/rollout-*.jsonl
        let root = dir.path().join(".codex");
        let day = root.join("sessions").join("2026").join("08").join("25");
        std::fs::create_dir_all(&day).unwrap();
        let path = day.join("rollout-2026-08-25T05-00-00-175821ac-a6a6-4c50-9487-67f90762b04a.jsonl");
        std::fs::write(&path, SAMPLE).unwrap();

        let fs = Arc::new(LocalFsProvider::new());
        let adapter = CodexAdapter::new();
        let entries = adapter.scan_sessions(&root, fs.as_ref());
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.session_id, "175821ac-a6a6-4c50-9487-67f90762b04a", "剥时间戳前缀");
        assert_eq!(e.project_path, "/Users/x/proj");

        let ok = |cwd: &str| cwd == "/Users/x/proj";
        let found = adapter.locate_session(&root, &e.session_id, fs.as_ref(), &ok);
        assert_eq!(found.as_ref(), Some(&path));

        let light = adapter.light_session(e, fs.as_ref()).unwrap();
        assert_eq!(light.agent, AgentKind::Codex);
        assert_eq!(light.first_message.as_deref(), Some("what is 2+2?"), "注入消息不作标题");
        // 4 = 真人 user + 注入 user + tool_result 载体 + 配对 assistant：
        // 严格对齐 claude 的 is_user_chunk_message 口径（isMeta 行也计数）
        assert_eq!(light.message_count, 4);
    }

    #[test]
    fn owns_path_matches_structure() {
        assert!(CodexAdapter::new().owns_path(Path::new(
            "/home/u/.codex/sessions/2026/08/25/rollout-x-uuid.jsonl"
        )));
        assert!(CodexAdapter::new().owns_path(Path::new(
            "/home/u/.codex/archived_sessions/rollout-x-uuid.jsonl"
        )));
        assert!(!CodexAdapter::new().owns_path(Path::new("/home/u/.claude/projects/-x-/a.jsonl")));
    }

    /// resolve_watch_event：rollout 文件读头 → (sid 剥前缀, cwd)。
    #[test]
    fn resolve_watch_event_reads_rollout_head() {
        let dir = tempfile::tempdir().unwrap();
        let day = dir.path().join("2026").join("08").join("25");
        std::fs::create_dir_all(&day).unwrap();
        let path = day.join("rollout-2026-08-25T05-00-00-175821ac-a6a6-4c50-9487-67f90762b04a.jsonl");
        std::fs::write(&path, SAMPLE).unwrap();
        let fs = Arc::new(LocalFsProvider::new());
        let (sid, cwd) = CodexAdapter::new()
            .resolve_watch_event(&path, fs.as_ref())
            .expect("rollout head should resolve");
        assert_eq!(sid, "175821ac-a6a6-4c50-9487-67f90762b04a");
        assert_eq!(cwd, "/Users/x/proj");
        // 非 rollout 命名 → None
        let other = day.join("notes.jsonl");
        std::fs::write(&other, "{}\n").unwrap();
        assert!(CodexAdapter::new().resolve_watch_event(&other, fs.as_ref()).is_none());
    }

    /// 真实数据 smoke（本机装有 codex 时）：`cargo test -- --ignored`
    #[test]
    #[ignore = "依赖本机 ~/.codex 真实数据"]
    fn real_data_smoke() {
        let home = dirs::home_dir().unwrap();
        let root = CodexAdapter::new().data_root_under(&home);
        if !root.join("sessions").is_dir() {
            eprintln!("no real codex data, skip");
            return;
        }
        let fs = Arc::new(LocalFsProvider::new());
        let adapter = CodexAdapter::new();
        let entries = adapter.scan_sessions(&root, fs.as_ref());
        eprintln!("scanned {} codex sessions", entries.len());
        assert!(!entries.is_empty());

        for e in &entries {
            let light = adapter
                .light_session(e, fs.as_ref())
                .unwrap_or_else(|| panic!("light failed: {:?}", e.file_path));
            let msgs = adapter.parse_messages(&e.file_path, fs.as_ref());
            let parsed = crate::parsing::process_messages(&msgs);
            let chunks = crate::analysis::ChunkBuilder::build_chunks(&msgs, &[]);
            // locate 往返（归属校验 = 自编码 id）
            let pid = e.project_id.clone();
            let ok = move |cwd: &str| encode_path(cwd) == pid;
            assert!(
                adapter
                    .locate_session(&root, &e.session_id, fs.as_ref(), &ok)
                    .is_some(),
                "locate miss: {}",
                e.session_id
            );
            eprintln!(
                "  {} → {} msgs, {} chunks, {} tokens, title={:?}",
                e.session_id,
                msgs.len(),
                chunks.len(),
                parsed.metrics.total_tokens,
                light.first_message.as_deref().unwrap_or(""),
            );
        }
    }
}

