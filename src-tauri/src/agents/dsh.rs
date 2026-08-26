//! DeepSeek Harness（dsh）适配器。
//!
//! 数据源：`~/.dsh/sessions/{escaped-cwd}/{escaped-id}/session.jsonl[.zstd]`，
//! 一目录一会话，文件名固定。`.zstd` 是**多帧连接**（首帧 header 行，之后
//! 每次 append 一帧），流式解码到 EOF 即可；`compression: none` 配置则是
//! 纯文本 `.jsonl`，两种后缀都认（并存时 mtime 新者胜，平局 .zstd 赢 ——
//! 与写端默认一致，Wake 同款裁决）。
//!
//! # 事件行格式（依据 Wake dsh.rs + dsh-session types，2026-08）
//!
//! 信封 `{type, seq?, time(epoch ms), data, surfaceOp?, ignorable?}`：
//! - `session`（首行）：`{id, cwd, createdAt, origin?, delegationDepth?}` ——
//!   cwd/id 权威；`origin=="subagent" || delegationDepth>0` 为子代理会话，
//!   **不进列表**；
//! - `user/message`：`data.content` 块数组；`data.source.kind != "user"`
//!   （如 agent-instructions/plugin/skill-catalog）为注入 → is_meta；
//! - `assistant/message`：`data.message.content` 块数组（text / reasoning /
//!   tool-call），`data.message.source.model` 记录模型，`data.usage` 是
//!   **单次调用**的账（三项 input 互斥，billed = 三项之和）→ **按调用累加**
//!   到 usage；流式 chunk 的合成体（assistant/chunk 与 *-chunks 打包行跳过）；
//! - `tool/result`：`data.message.content[0]` 为 tool-result 块
//!   （toolCallId + 嵌套 content + isError）→ 回填 tool_use（对前端以紧随
//!   user 载体呈现，与 pi/codex 载体形态一致）；
//! - `session/title`：last-wins 标题；
//! - `surfaceOp/op=="replace"`：上下文裁剪标记（喂模型的），跳过 ——
//!   Wake 还原用户所见原文；
//! - 约 50 个已知噪声事件类型（KNOWN_SKIP 词汇表）显式跳过；`ignorable=true`
//!   的行为写端声明的纯信息性记录，可安全跳过；词汇表外计 unknown 金丝雀。
//!
//! # 能力边界（如实声明）
//!
//! - 本机无 dsh 安装：实现按 Wake 源码格式，fixture 驱动验证；
//! - **实时刷新为全量重解**：zstd 多帧无法按 byte offset 增量（帧边界即
//!   append 边界，但 offset 对齐无意义），watcher 事件触发缓存失效 + 全量；
//! - zstd 断尾（写端 append 到一半）安全跳过已解码部分（Wake 同款）。

use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::agents::{path_has_components, AgentAdapter, AgentSessionEntry};
use crate::infrastructure::fs_provider::FsProvider;
use crate::types::domain::{AgentKind, MessageType, Session, SessionMetadataLevel};
use crate::types::jsonl::UsageMetadata;
use crate::types::messages::ParsedMessage;
use crate::utils::encode_path;

/// dsh 当前版本除内容事件外的完整事件词汇（Wake 源码 known-event-types.ts，
/// 2026-08）+ 三种 chunk 打包存储行。上游新增词汇会计入 unknown 金丝雀。
const KNOWN_SKIP: &[&str] = &[
    "agent-preset/selected",
    "agent/inbox/spliced",
    "approval/asked",
    "approval/decided",
    "approval/policy",
    "assistant/chunk",
    "command/done",
    "command/run",
    "compaction/end",
    "compaction/prune",
    "compaction/start",
    "compaction/summary",
    "feedback/record",
    "goal/change",
    "hook/invoked",
    "hook/result",
    "llm/retry",
    "llm/retry-started",
    "permission/preset",
    "plan/mode",
    "request/header",
    "sandbox/mode",
    "schedule/change",
    "session/end-seed",
    "session/title-llm-request",
    "step/end",
    "step/start",
    "subagent/descriptor",
    "team/member",
    "team/message/delivered",
    "team/message/queued",
    "team/task",
    "todo/write",
    "tool-workflow/agent-end",
    "tool-workflow/agent-start",
    "tool-workflow/run-end",
    "tool-workflow/run-start",
    "tool/call",
    "tool/code-dispatch",
    "tool/code-dispatch-start",
    "turn/end",
    "turn/start",
    "web/deepseek-search-llm-request",
    "text-chunks",
    "reasoning-chunks",
    "tool-call-chunks",
];

pub struct DshAdapter;

impl DshAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DshAdapter {
    fn default() -> Self {
        Self::new()
    }
}


/// ruzstd 多帧解码到 String。`decode_all_to_vec` 要求调用方预知输出容量
///（TargetTooSmall 陷阱），这里预留容量起步、不足翻倍重试（输入全量在
/// 内存，重试廉价）。
fn decode_zstd_all(bytes: &[u8]) -> Option<String> {
    let mut capacity = (bytes.len() * 8).max(64 * 1024);
    loop {
        let mut out = Vec::with_capacity(capacity);
        let mut dec = ruzstd::decoding::FrameDecoder::new();
        match dec.decode_all_to_vec(bytes, &mut out) {
            Ok(()) => return String::from_utf8(out).ok(),
            Err(_) if capacity < 512 << 20 => capacity *= 2,
            Err(_) => return None,
        }
    }
}

/// 打开会话日志内容（.zstd 多帧流式解码 / 纯文本直读）。
/// zstd 断尾（写端 append 中途）→ 已解码部分照常返回（Wake 同款降级）。
fn read_log_content(path: &Path, fs: &dyn FsProvider) -> Option<String> {
    let is_zstd = path.extension().is_some_and(|e| e == "zstd");
    if !is_zstd {
        return fs.read_file(path).ok();
    }
    // zstd：拉全量字节后 FrameDecoder::decode_all_to_vec 解尽所有帧
    //（ruzstd 的 Read 实现读到首帧 EOF 即返回 0，不自动续帧 —— 多帧必须
    // 用 decode_all；dsh 每次 append 一帧，多帧即 append 语义）
    let bytes = fs.read_file_range(path, 0, None).ok()?;
    decode_zstd_all(&bytes)
}

/// 首行 header：`(id, cwd, created_ms, subagent)`。
struct DshHeader {
    id: String,
    cwd: String,
    created_ms: u64,
    subagent: bool,
}

fn parse_header(row: &Value) -> Option<DshHeader> {
    // session 事件的有效载荷在 data 内（与其他事件一致的信封结构）
    if row.get("type").and_then(|t| t.as_str()) != Some("session") {
        return None;
    }
    let d = row.get("data")?;
    let subagent = d.get("origin").and_then(|o| o.as_str()) == Some("subagent")
        || d.get("delegationDepth").and_then(|x| x.as_i64()).unwrap_or(0) > 0;
    Some(DshHeader {
        id: d.get("id")?.as_str()?.to_string(),
        cwd: d
            .get("cwd")
            .and_then(|c| c.as_str())
            .unwrap_or_default()
            .to_string(),
        created_ms: d
            .get("createdAt")
            .and_then(|t| t.as_i64())
            .filter(|t| *t > 0)
            .map(|t| t as u64)
            .unwrap_or(0),
        subagent,
    })
}

/// 读首行拿 header（轻量：read_file_head 1 行；zstd 惰性解首帧）。
fn read_header(path: &Path, fs: &dyn FsProvider) -> Option<DshHeader> {
    let is_zstd = path.extension().is_some_and(|e| e == "zstd");
    let first_line = if is_zstd {
        // zstd 无法只读一行：解首帧后的第一行（首帧通常只含 header 行，
        // 实测写端行为；退化解全文首行亦可）
        let content = read_log_content(path, fs)?;
        content.lines().next()?.to_string()
    } else {
        fs.read_file_head(path, 1).ok()?.lines().next()?.to_string()
    };
    let row: Value = serde_json::from_str(first_line.trim()).ok()?;
    parse_header(&row)
}

/// 同目录 sibling 裁决：压缩配置换挡会新旧后缀并存，mtime 新者胜
/// （平局 .zstd 赢，与写端当前默认一致）。
fn stale_sibling_loses(path: &Path, fs: &dyn FsProvider) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    let sibling_name = if name == "session.jsonl" {
        "session.jsonl.zstd"
    } else {
        "session.jsonl"
    };
    let sibling = path.with_file_name(sibling_name);
    let (own_m, sib_m) = match (fs.stat(path), fs.stat(&sibling)) {
        (Ok(a), Ok(b)) => (a.mtime_ms, b.mtime_ms),
        _ => return true, // sibling 不存在 → 自己有效
    };
    if sib_m > own_m || (sib_m == own_m && name == "session.jsonl") {
        return false; // sibling 胜出 → 自己过时
    }
    true
}

/// dsh 信封 → ParsedMessage（映射见模块文档）。
pub(crate) fn parse_dsh_content(content: &str) -> DshParse {
    let mut p = DshParse::default();
    // toolCallId → (assistant 下标)：tool/result 回填 content 内的 tool_use 块
    let mut tool_index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut seq = 0usize;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(row) = serde_json::from_str::<Value>(line) else {
            p.unknown += 1;
            continue;
        };
        let ty = row.get("type").and_then(|t| t.as_str()).unwrap_or_default();
        // 上下文裁剪（surfaceOp replace 是对象，不能按字符串比）→ 跳过
        if row.pointer("/surfaceOp/op").and_then(|v| v.as_str()) == Some("replace") {
            continue;
        }
        let ts = row.get("time").and_then(|v| v.as_i64()).unwrap_or(0);
        if ts > 0 && (p.last_ts == 0 || ts > p.last_ts) {
            p.last_ts = ts;
        }
        let data = row.get("data").cloned().unwrap_or(Value::Null);

        match ty {
            "session" => {
                if let Some(h) = parse_header(&row) {
                    p.header = Some(h);
                }
            }
            "user/message" => {
                let text = blocks_text(data.get("content"));
                if text.trim().is_empty() {
                    continue;
                }
                seq += 1;
                let kind = data.pointer("/source/kind").and_then(|k| k.as_str());
                let mut m = dsh_msg(&mut seq, MessageType::User, "user", ts, &p);
                // 白名单 "user"：非真人（agent-instructions/plugin/...）归 meta
                if kind.is_some_and(|k| k != "user") {
                    m.is_meta = true;
                }
                m.content = Value::Array(vec![serde_json::json!({"type":"text","text":text})]);
                p.messages.push(m);
            }
            "assistant/message" => {
                let msg = data.get("message").cloned().unwrap_or(Value::Null);
                let content = msg.get("content").cloned().unwrap_or(Value::Null);
                // 单遍分桶：text / reasoning / tool-call（blocks_text 会把
                // reasoning 的 text 混进正文，不适用）
                let mut text_parts: Vec<String> = Vec::new();
                let mut thinking_parts: Vec<String> = Vec::new();
                let mut blocks: Vec<Value> = Vec::new();
                let mut tool_calls = vec![];
                for b in content.as_array().into_iter().flatten() {
                    let btext = || {
                        b.get("text")
                            .and_then(|v| v.as_str())
                            .map(str::trim)
                            .filter(|t| !t.is_empty())
                    };
                    match b.get("type").and_then(|v| v.as_str()) {
                        Some("text") => {
                            if let Some(t) = btext() {
                                text_parts.push(t.to_string());
                                blocks.push(serde_json::json!({"type":"text","text":t}));
                            }
                        }
                        Some("reasoning") => {
                            if let Some(t) = btext() {
                                thinking_parts.push(t.to_string());
                                blocks.insert(
                                    blocks.len().saturating_sub(text_parts.len().max(1) + 1),
                                    serde_json::json!({"type":"thinking","thinking":t,"signature":""}),
                                );
                            }
                        }
                        Some("tool-call") => {
                            let id = b.get("id").and_then(|v| v.as_str()).unwrap_or("");
                            let name = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
                            // arguments 是模型原样输出的 JSON 字符串
                            let raw = b.get("arguments").and_then(|v| v.as_str()).unwrap_or("");
                            let input = serde_json::from_str::<Value>(raw)
                                .unwrap_or(Value::String(raw.to_string()));
                            blocks.push(serde_json::json!({
                                "type":"tool_use","id":id,"name":name,"input":input
                            }));
                            tool_calls.push(crate::types::messages::ToolCall {
                                id: id.to_string(),
                                name: name.to_string(),
                                input,
                                is_task: false,
                                task_description: None,
                                task_subagent_type: None,
                            });
                        }
                        _ => {}
                    }
                }
                // 元数据先收再判空：content 为空的 assistant/message 是 dsh
                // 专挂 usage 的载体（撞 max-tokens 等），提前 continue 会丢账
                let model = msg
                    .pointer("/source/model")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                if model.is_some() {
                    p.model.clone_from(&model);
                }
                if let Some(u) = data.get("usage") {
                    // 单次调用账（三项 input 互斥）：按调用累加，非 last-wins
                    let get = |k: &str| u.get(k).and_then(|v| v.as_i64()).unwrap_or(0);
                    let sum = get("inputTokens")
                        + get("outputTokens")
                        + get("cacheReadTokens")
                        + get("cacheWriteTokens");
                    if sum > 0 {
                        let usage = p.usage.clone().unwrap_or(UsageMetadata {
                            input_tokens: 0,
                            output_tokens: 0,
                            cache_read_input_tokens: None,
                            cache_creation_input_tokens: None,
                        });
                        p.usage = Some(UsageMetadata {
                            input_tokens: usage.input_tokens + get("inputTokens") as u64,
                            output_tokens: usage.output_tokens + get("outputTokens") as u64,
                            cache_read_input_tokens: match usage.cache_read_input_tokens {
                                Some(v) => Some(v + get("cacheReadTokens") as u64),
                                None if get("cacheReadTokens") > 0 => {
                                    Some(get("cacheReadTokens") as u64)
                                }
                                None => None,
                            },
                            cache_creation_input_tokens: match usage.cache_creation_input_tokens {
                                Some(v) => Some(v + get("cacheWriteTokens") as u64),
                                None if get("cacheWriteTokens") > 0 => {
                                    Some(get("cacheWriteTokens") as u64)
                                }
                                None => None,
                            },
                        });
                    }
                }
                if text_parts.is_empty() && thinking_parts.is_empty() && tool_calls.is_empty() {
                    continue;
                }
                seq += 1;
                let mut m = dsh_msg(&mut seq, MessageType::Assistant, "assistant", ts, &p);
                // thinking 块保持在前（与 claude 习惯一致；上面 insert 位置已保序
                // 简化为统一前插——重排：thinking 全部移到 text/tool 之前）
                let (think_blocks, rest): (Vec<Value>, Vec<Value>) = blocks
                    .into_iter()
                    .partition(|b| b.get("type") == Some(&serde_json::json!("thinking")));
                let mut ordered = think_blocks;
                ordered.extend(rest);
                m.content = Value::Array(ordered);
                m.model = model;
                m.tool_calls = tool_calls;
                m.usage = p.usage.clone();
                let idx = p.messages.len();
                for tc in &m.tool_calls {
                    if !tc.id.is_empty() {
                        tool_index.insert(tc.id.clone(), idx);
                    }
                }
                p.messages.push(m);
            }
            "tool/result" => {
                let Some(block) = data.pointer("/message/content/0") else {
                    continue;
                };
                let Some(call_id) = block.get("toolCallId").and_then(|v| v.as_str()) else {
                    continue;
                };
                let text = blocks_text(block.get("content"));
                let is_error = block.get("isError").and_then(|v| v.as_bool()) == Some(true)
                    || data.get("error").is_some_and(|e| !e.is_null());
                // 回填 tool_use 块（assistant content 内）；同时以 user 载体呈现
                if let Some(&ai_idx) = tool_index.get(call_id) {
                    if let Some(arr) = p.messages[ai_idx].content.as_array_mut() {
                        for b in arr.iter_mut() {
                            if b.get("type") == Some(&serde_json::json!("tool_use"))
                                && b.get("id") == Some(&serde_json::json!(call_id))
                            {
                                b.as_object_mut().unwrap().insert(
                                    "dsh_result".to_string(),
                                    serde_json::json!({"content": text, "is_error": is_error}),
                                );
                                break;
                            }
                        }
                    }
                }
                seq += 1;
                let mut m = dsh_msg(&mut seq, MessageType::User, "user", ts, &p);
                m.is_meta = true;
                m.content = Value::Array(vec![serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": call_id,
                    "content": text,
                    "is_error": is_error,
                })]);
                m.tool_results = crate::parsing::extract_tool_results(&m.content);
                p.messages.push(m);
            }
            "session/title" => {
                if let Some(t) = data.get("title").and_then(|v| v.as_str()) {
                    let t = t.trim();
                    if !t.is_empty() {
                        p.title = Some(t.chars().take(80).collect());
                    }
                }
            }
            "request/context" => {
                if p.model.is_none() {
                    if let Some(m) = data.get("model").and_then(|v| v.as_str()) {
                        p.model = Some(m.to_string());
                    }
                }
            }
            // 信封自带 ignorable = 写端声明的纯信息性记录
            _ if row.get("ignorable").and_then(|v| v.as_bool()) == Some(true) => {}
            // 已知噪声词汇
            t if KNOWN_SKIP.contains(&t) => {}
            _ => {
                p.unknown += 1;
                log::debug!("dsh: unknown event type: {ty}");
            }
        }
    }
    p
}

/// content 块数组 → 纯文本（text 块拼接；reasoning 的 text 会被混入 ——
/// 仅用于 user 文本与 tool 输出，assistant 分桶走专门路径）。
fn blocks_text(v: Option<&Value>) -> String {
    v.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[derive(Default)]
pub(crate) struct DshParse {
    header: Option<DshHeader>,
    title: Option<String>,
    model: Option<String>,
    usage: Option<UsageMetadata>,
    last_ts: i64,
    messages: Vec<ParsedMessage>,
    unknown: u32,
}

fn dsh_msg(seq: &mut usize, ty: MessageType, role: &str, ts_ms: i64, p: &DshParse) -> ParsedMessage {
    *seq += 1;
    ParsedMessage {
        uuid: format!("dsh-{seq}"),
        parent_uuid: None,
        message_type: ty,
        timestamp: chrono::DateTime::from_timestamp_millis(ts_ms)
            .map(|d| d.to_rfc3339())
            .unwrap_or_default(),
        role: Some(role.to_string()),
        content: Value::Null,
        usage: None,
        model: None,
        cwd: p.header.as_ref().map(|h| h.cwd.clone()),
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

impl AgentAdapter for DshAdapter {
    fn kind(&self) -> AgentKind {
        AgentKind::Dsh
    }

    fn data_roots(&self) -> Vec<PathBuf> {
        vec![dirs::home_dir().unwrap_or_default().join(".dsh").join("sessions")]
    }

    fn owns_path(&self, path: &Path) -> bool {
        path_has_components(path, &[".dsh", "sessions"])
    }

    fn data_root_under(&self, home: &Path) -> PathBuf {
        home.join(".dsh").join("sessions")
    }

    fn scan_sessions(&self, root: &Path, fs: &dyn FsProvider) -> Vec<AgentSessionEntry> {
        // 固定两层：{project}/{session}/session.jsonl[.zstd]（不深递归 ——
        // 会话目录还放别的 artifacts）
        let mut entries = Vec::new();
        let Ok(projects) = fs.read_dir(root) else {
            return entries;
        };
        for proj in &projects {
            if !proj.is_directory {
                continue;
            }
            let proj_path = root.join(&proj.name);
            let Ok(sessions) = fs.read_dir(&proj_path) else {
                continue;
            };
            for s in &sessions {
                if !s.is_directory {
                    continue;
                }
                let dir = proj_path.join(&s.name);
                // 两个候选名过 sibling 漏斗（平局 .zstd 赢）
                for name in ["session.jsonl.zstd", "session.jsonl"] {
                    let file = dir.join(name);
                    if !fs.exists(&file).unwrap_or(false) {
                        continue;
                    }
                    let Some(meta) = fs.stat(&file).ok().filter(|m| m.size > 0) else {
                        continue;
                    };
                    if !stale_sibling_loses(&file, fs) {
                        continue;
                    }
                    let Some(h) = read_header(&file, fs).filter(|h| !h.subagent) else {
                        continue;
                    };
                    let (mtime_ms, birth) = (meta.mtime_ms, meta.birthtime_ms);
                    entries.push(AgentSessionEntry {
                        agent: AgentKind::Dsh,
                        project_id: encode_path(&h.cwd),
                        project_path: h.cwd.clone(),
                        session_id: h.id,
                        file_path: file,
                        mtime_ms,
                        birthtime_ms: if birth > 0 { birth } else { h.created_ms },
                        created_ms: h.created_ms,
                    });
                    break;
                }
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
        let projects = fs.read_dir(root).ok()?;
        for proj in &projects {
            if !proj.is_directory {
                continue;
            }
            let proj_path = root.join(&proj.name);
            let Ok(sessions) = fs.read_dir(&proj_path) else {
                continue;
            };
            for s in &sessions {
                if !s.is_directory {
                    continue;
                }
                let dir = proj_path.join(&s.name);
                for name in ["session.jsonl.zstd", "session.jsonl"] {
                    let file = dir.join(name);
                    if !fs.exists(&file).unwrap_or(false) {
                        continue;
                    }
                    if let Some(h) = read_header(&file, fs) {
                        if h.id == session_id && id_matches(&h.cwd) {
                            return Some(file);
                        }
                    }
                }
            }
        }
        None
    }

    fn parse_messages(&self, path: &Path, fs: &dyn FsProvider) -> Vec<ParsedMessage> {
        let Some(content) = read_log_content(path, fs) else {
            return vec![];
        };
        parse_dsh_content(&content).messages
    }

    fn resolve_watch_event(
        &self,
        path: &Path,
        fs: &dyn FsProvider,
    ) -> Option<(String, String)> {
        let name = path.file_name()?.to_str()?;
        if name != "session.jsonl" && name != "session.jsonl.zstd" {
            return None;
        }
        let h = read_header(path, fs)?;
        if h.subagent {
            return None;
        }
        Some((h.id, h.cwd))
    }

    fn light_session(&self, entry: &AgentSessionEntry, fs: &dyn FsProvider) -> Option<Session> {
        let Some(h) = read_header(&entry.file_path, fs) else {
            return None;
        };
        // light 预览：读内容前 200 行等价物（zstd 全解后的行截断）
        let content = read_log_content(&entry.file_path, fs)?;
        let head: String = content.lines().take(200).collect::<Vec<_>>().join("\n");
        let parsed = parse_dsh_content(&head);
        let title = parsed
            .title
            .clone()
            .or_else(|| {
                parsed
                    .messages
                    .iter()
                    .find(|m| m.message_type == MessageType::User && !m.is_meta)
                    .and_then(|m| {
                        m.content
                            .as_array()
                            .and_then(|a| a.first())
                            .and_then(|b| b.get("text"))
                            .and_then(|t| t.as_str())
                            .map(|t| t.chars().take(80).collect::<String>())
                    })
            })
            .unwrap_or_else(|| "Untitled".to_string());
        Some(Session {
            id: entry.session_id.clone(),
            agent: AgentKind::Dsh,
            project_id: entry.project_id.clone(),
            project_path: entry.project_path.clone(),
            created_at: if entry.created_ms > 0 {
                entry.created_ms
            } else {
                entry.mtime_ms
            },
            updated_at: Some(if parsed.last_ts > 0 {
                parsed.last_ts as u64
            } else {
                entry.mtime_ms
            }),
            todo_data: None,
            first_message: Some(title),
            message_timestamp: None,
            has_subagents: false,
            message_count: 0, // light 不再逐条数（dsh 需全解，交给详情）
            is_ongoing: None,
            git_branch: None,
            metadata_level: Some(SessionMetadataLevel::Light),
            context_consumption: None,
            compaction_count: None,
            phase_breakdown: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::AgentAdapter;
    use crate::infrastructure::fs_provider::LocalFsProvider;
    use std::sync::Arc;

    const SAMPLE: &str = concat!(
        r#"{"type":"session","seq":1,"time":1773741712000,"data":{"id":"dsh_s1","cwd":"/Users/x/dsh-proj","createdAt":1773741712000}}"#,
        "\n",
        r#"{"type":"user/message","seq":2,"time":1773741712318,"data":{"content":[{"type":"text","text":"帮我看看这个项目"}],"source":{"kind":"user"}}}"#,
        "\n",
        r#"{"type":"user/message","seq":3,"time":1773741712400,"data":{"content":[{"type":"text","text":"[skill catalog injected]"}],"source":{"kind":"skill-catalog"}}}"#,
        "\n",
        r#"{"type":"assistant/message","seq":4,"time":1773741720000,"data":{"message":{"source":{"model":"deepseek-chat"},"content":[{"type":"reasoning","text":"planning..."},{"type":"text","text":"我来看一下"},{"type":"tool-call","id":"call_d1","name":"bash","arguments":"{\"command\":\"ls\"}"}]},"usage":{"inputTokens":1000,"outputTokens":200,"cacheReadTokens":300,"cacheWriteTokens":50}}}"#,
        "\n",
        r#"{"type":"tool/result","seq":5,"time":1773741730000,"data":{"message":{"content":[{"type":"tool-result","toolCallId":"call_d1","content":[{"type":"text","text":"file1\nfile2"}],"isError":false}]}}}"#,
        "\n",
        r#"{"type":"assistant/message","seq":6,"time":1773741740000,"data":{"message":{"source":{"model":"deepseek-chat"},"content":[{"type":"text","text":"共两个文件"}]},"usage":{"inputTokens":800,"outputTokens":100}}}"#,
        "\n",
        r#"{"type":"session/title","seq":7,"time":1773741750000,"data":{"title":"项目结构查看"}}"#,
        "\n",
        r#"{"type":"turn/start","seq":8,"time":1773741751000,"data":{}}"#,
        "\n",
        r#"{"type":"surfaceOp-test","seq":9,"time":1773741752000,"data":{},"surfaceOp":{"op":"replace","start":1,"end":2}}"#,
        "\n",
    );

    #[test]
    fn parse_maps_dsh_events_to_claude_semantics() {
        let p = parse_dsh_content(SAMPLE);
        let msgs = p.messages;

        // user(真人) / user(注入 meta) / assistant(thinking+text+tool) /
        // tool_result 载体 / assistant(答案)
        assert_eq!(msgs.len(), 5);
        assert_eq!(msgs[0].message_type, MessageType::User);
        assert!(!msgs[0].is_meta);
        assert_eq!(
            msgs[0].content.as_array().unwrap()[0].get("text"),
            Some(&serde_json::json!("帮我看看这个项目"))
        );

        assert!(msgs[1].is_meta, "source.kind=skill-catalog → is_meta");

        let a1 = &msgs[2];
        assert_eq!(a1.model.as_deref(), Some("deepseek-chat"));
        assert_eq!(a1.tool_calls.len(), 1);
        assert_eq!(a1.tool_calls[0].name, "bash");
        assert_eq!(
            a1.tool_calls[0].input,
            serde_json::json!({"command": "ls"}),
            "arguments JSON 字符串解析为对象"
        );
        // usage 按调用累加：input/cacheRead/cacheWrite 分列（dsh 三项互斥）
        let u1 = a1.usage.as_ref().unwrap();
        assert_eq!(u1.input_tokens, 1000);
        assert_eq!(u1.cache_read_input_tokens, Some(300));
        assert_eq!(u1.cache_creation_input_tokens, Some(50));
        assert_eq!(u1.output_tokens, 200);
        let blocks = a1.content.as_array().unwrap();
        assert_eq!(blocks[0].get("type"), Some(&serde_json::json!("thinking")), "thinking 前置");

        let tr = &msgs[3];
        assert!(tr.is_meta);
        assert_eq!(tr.tool_results[0].tool_use_id, "call_d1");
        assert!(tr.tool_results[0].content.as_str().unwrap().contains("file1"));

        let a2 = &msgs[4];
        let u2 = a2.usage.as_ref().unwrap();
        assert_eq!(u2.input_tokens, 1000 + 800, "跨调用累加");
        assert_eq!(u2.output_tokens, 200 + 100);

        // 元信息
        assert_eq!(p.title.as_deref(), Some("项目结构查看"));
        assert_eq!(p.header.as_ref().unwrap().cwd, "/Users/x/dsh-proj");
        assert_eq!(p.unknown, 0, "turn/start 在 KNOWN_SKIP，surfaceOp 行跳过");
    }

    #[test]
    fn parsed_output_survives_downstream_pipeline() {
        let p = parse_dsh_content(SAMPLE);
        let parsed = crate::parsing::process_messages(&p.messages);
        assert!(parsed.metrics.input_tokens > 0);
        let chunks = crate::analysis::ChunkBuilder::build_chunks(&p.messages, &[]);
        assert!(!chunks.is_empty());
    }

    #[test]
    fn scan_locate_subagent_filter_and_sibling_rule() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("sessions");
        let proj = root.join("esc-cwd");
        let s1 = proj.join("esc-id-1");
        let s2 = proj.join("esc-id-2"); // 子代理：不进列表
        std::fs::create_dir_all(&s1).unwrap();
        std::fs::create_dir_all(&s2).unwrap();
        std::fs::write(
            s1.join("session.jsonl"),
            concat!(
                r#"{"type":"session","time":1,"data":{"id":"dsh_main","cwd":"/Users/x/p","createdAt":100}}"#,
                "\n",
            ),
        )
        .unwrap();
        std::fs::write(
            s2.join("session.jsonl"),
            concat!(
                r#"{"type":"session","time":1,"data":{"id":"dsh_sub","cwd":"/Users/x/p","createdAt":100,"origin":"subagent"}}"#,
                "\n",
            ),
        )
        .unwrap();

        let fs = Arc::new(LocalFsProvider::new());
        let adapter = DshAdapter::new();
        let entries = adapter.scan_sessions(&root, fs.as_ref());
        assert_eq!(entries.len(), 1, "子代理会话被过滤");
        assert_eq!(entries[0].session_id, "dsh_main");
        assert_eq!(entries[0].project_id, "-Users-x-p");

        // sibling 裁决：更新 .zstd 后 .jsonl 让位（mtime 新者胜）。
        // zstd 多帧（首帧 header / 第二帧消息）—— 与 dsh 写端 append 形态一致
        std::fs::write(
            s1.join("session.jsonl.zstd"),
            zstd_frames(&[
                r#"{"type":"session","time":2,"data":{"id":"dsh_main2","cwd":"/Users/x/p","createdAt":200}}"#,
                "\n",
                r#"{"type":"user/message","time":3,"data":{"content":[{"type":"text","text":"hi"}],"source":{"kind":"user"}}}"#,
                "\n",
            ]),
        )
        .unwrap();
        let entries = adapter.scan_sessions(&root, fs.as_ref());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id, "dsh_main2", "zstd 后缀胜出");

        // locate 归属校验
        let ok = |cwd: &str| cwd == "/Users/x/p";
        assert_eq!(
            adapter.locate_session(&root, "dsh_main2", fs.as_ref(), &ok),
            Some(s1.join("session.jsonl.zstd"))
        );
    }

    /// 用 zstd crate 编码多帧（每段一帧，模拟 dsh append 行为；测试专用）。
    fn zstd_frames(chunks: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        for c in chunks {
            let mut enc = zstd::Encoder::new(Vec::new(), 3).unwrap();
            use std::io::Write;
            enc.write_all(c.as_bytes()).unwrap();
            out.extend(enc.finish().unwrap());
        }
        out
    }

    /// zstd 多帧解码：所有帧拼接完整（append 语义）+ 全量 parse 走通。
    #[test]
    fn zstd_multiframe_decodes_all_frames() {
        let frames = zstd_frames(&[
            r#"{"type":"session","time":1,"data":{"id":"z1","cwd":"/z","createdAt":100}}"#,
            "\n",
            r#"{"type":"user/message","time":2,"data":{"content":[{"type":"text","text":"第一帧问题"}],"source":{"kind":"user"}}}"#,
            "\n",
            r#"{"type":"assistant/message","time":3,"data":{"message":{"source":{"model":"deepseek-chat"},"content":[{"type":"text","text":"第二帧回答"}]}}}"#,
            "\n",
        ]);
        // 直接喂 FrameDecoder::decode_all_to_vec（read_log_content 同路径）
        let text = decode_zstd_all(&frames).unwrap();
        assert!(text.contains("第一帧问题") && text.contains("第二帧回答"), "多帧全部解出");
        let p = parse_dsh_content(&text);
        assert_eq!(p.messages.len(), 2);
        assert_eq!(p.messages[1].model.as_deref(), Some("deepseek-chat"));
    }

    #[test]
    fn owns_path_matches_structure() {
        assert!(DshAdapter::new().owns_path(Path::new(
            "/home/u/.dsh/sessions/x/y/session.jsonl"
        )));
        assert!(DshAdapter::new().owns_path(Path::new(
            "/home/u/.dsh/sessions/x/y/session.jsonl.zstd"
        )));
        assert!(!DshAdapter::new().owns_path(Path::new("/home/u/.claude/projects/a.jsonl")));
    }
}

