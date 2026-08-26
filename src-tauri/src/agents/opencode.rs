//! OpenCode 适配器（SQLite 型）。
//!
//! 数据源：`~/.local/share/opencode/{opencode.db, opencode-next.db}`
//! （XDG_DATA_HOME 指向含库文件的目录时采信；OPENCODE_DB 为额外候选 ——
//! 仅本地上下文生效，同 CODEX_HOME 模式）。
//!
//! # 寻址：虚拟路径
//!
//! SQLite 无会话文件，统一寻址用虚拟路径 `{db_path}#{session_id}`
//!（Wake 同款）。`parse_messages` 按 `#` 拆解后打开 DB 查询；`owns_path`
//! 按库文件名结构特征判定。
//!
//! # Schema（2026-08 本机实测 v1：146 会话 / 1802 message / 9364 part）
//!
//! - `session`：`id(ses_xxx), parent_id(非空=子代理，跳过), directory(权威
//!   cwd), title(质量差，如 "Background: undefined" —— 仅作回退), version,
//!   time_created/time_updated(**epoch ms INTEGER**)`。无 model/tokens 列
//!   （在 message data JSON 里）；
//! - `message`：`id(msg_xxx), session_id, time_created, data(JSON:
//!   {role, time.created, modelID, providerID, agent})`；
//! - `part`：`id, message_id, session_id, time_created, data(JSON:
//!   {type, ...})` —— 无独立 type/synthetic 列，全在 data 内：
//!   - `text`：`{text, synthetic?}`（synthetic=true 是注入内容：文件树/
//!     编辑器上下文 → is_meta 折叠）；
//!   - `reasoning`：`{text}` → thinking 块；
//!   - `tool`：`{callID, tool, state:{status, input, output}}` →
//!     tool_use 块（input=state.input）+ **output 也在 state 内** → 映射为
//!     紧随的 user 载体 tool_result（is_meta，与 pi/codex 载体形态一致）；
//!   - `step-start/step-finish/snapshot/patch/file`：噪声；
//!   - `step-finish.tokens`：`{total, input, output, reasoning,
//!     cache:{read, write}}` → 挂所属 assistant 的 usage（累加）；
//! - OpenCode 2 next（`session_message` 表，type 列 user/synthetic/
//!   assistant/shell + data JSON）：运行时表探测切换解析路径。本机无
//!   v2 数据，该分支按 Wake 实测格式实现，fixture 驱动验证。
//!
//! # 能力边界（如实声明）
//!
//! - **只读**：`Connection::open_with_flags(READ_ONLY)`，绝不写别人的库 →
//!   会话删除 no-op（删除虚拟路径文件不存在，安全返回）；
//! - **SSH 模式不支持**：rusqlite 需要本地随机读，SFTP 上逐页拉等于灾难
//!   （既定决策，`provider_type=="ssh"` 时全部空实现 + info 日志）；
//! - **无实时刷新**：watcher 只认 .jsonl，库文件 mtime 变化不触发事件
//!   —— 列表靠 120s TTL 兜底刷新（对齐 Wake 对 SQLite 型的观察）。

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde_json::Value;

use crate::agents::{path_has_components, AgentAdapter, AgentSessionEntry};
use crate::infrastructure::fs_provider::FsProvider;
use crate::types::domain::{AgentKind, MessageType, Session, SessionMetadataLevel};
use crate::types::jsonl::UsageMetadata;
use crate::types::messages::ParsedMessage;
use crate::utils::encode_path;

const DB_NAMES: [&str; 2] = ["opencode.db", "opencode-next.db"];

pub struct OpencodeAdapter {
    /// 本地 env 候选（XDG_DATA_HOME / OPENCODE_DB；仅本地上下文生效）。
    extra_dirs: Vec<PathBuf>,
    /// SSH 降级提示只打一次。
    ssh_notice: std::sync::atomic::AtomicBool,
}

impl OpencodeAdapter {
    pub fn new() -> Self {
        let mut extra_dirs = Vec::new();
        if let Some(xdg) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
            extra_dirs.push(PathBuf::from(xdg).join("opencode"));
        }
        if let Some(db) = std::env::var_os("OPENCODE_DB").filter(|v| !v.is_empty()) {
            extra_dirs.push(PathBuf::from(db));
        }
        Self {
            extra_dirs,
            ssh_notice: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// 候选库目录：默认 ~/.local/share/opencode + env 候选（去重）。
    fn db_dirs(&self, home: &Path) -> Vec<PathBuf> {
        let mut dirs = vec![home.join(".local").join("share").join("opencode")];
        for d in &self.extra_dirs {
            if !dirs.contains(d) {
                dirs.push(d.clone());
            }
        }
        dirs
    }

    /// SSH 降级检查：不支持即空（info 只打一次）。
    fn ssh_guard(&self, fs: &dyn FsProvider) -> bool {
        if fs.provider_type() == "ssh" {
            if !self
                .ssh_notice
                .swap(true, std::sync::atomic::Ordering::Relaxed)
            {
                log::info!(
                    "agents: opencode sessions unavailable over SSH (sqlite requires local access)"
                );
            }
            false
        } else {
            true
        }
    }
}

impl Default for OpencodeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

// ── 虚拟路径 ────────────────────────────────────────────────────────

/// `{db_path}#{session_id}` → (db_path, session_id)。`#` 不出现在路径中
///（Wake 同款约束，path_owns 边界同理）。
fn parse_virtual_path(path: &Path) -> Option<(PathBuf, String)> {
    let s = path.to_str()?;
    let (db, sid) = s.split_once('#')?;
    if db.is_empty() || sid.is_empty() {
        return None;
    }
    Some((PathBuf::from(db), sid.to_string()))
}

fn virtual_path(db: &Path, session_id: &str) -> PathBuf {
    PathBuf::from(format!("{}#{}", db.display(), session_id))
}

/// 只读打开（绝不写别人的库；不用 immutable —— WAL 下会读到不一致快照）。
fn open_ro(db: &Path) -> Option<Connection> {
    Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()
}

// ── schema 探测 ────────────────────────────────────────────────────

fn has_table(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |_| Ok(()),
    )
    .is_ok()
}

/// 会话正文在 v2（session_message 表）还是 v1（message+part 两表）。
fn uses_v2(conn: &Connection) -> bool {
    has_table(conn, "session_message")
}

fn session_table(conn: &Connection) -> &'static str {
    if has_table(conn, "session_v2") {
        "session_v2"
    } else {
        "session"
    }
}

// ── 元数据行 ───────────────────────────────────────────────────────

struct OcRow {
    id: String,
    directory: String,
    title: Option<String>,
    parent_id: Option<String>,
    created_ms: i64,
    updated_ms: i64,
}

fn query_rows(conn: &Connection, id: Option<&str>) -> rusqlite::Result<Vec<OcRow>> {
    // parent_id IS NULL 过滤子代理；列名兼容 v1（本机实测）与 Wake 观察的
    // 稳定版（同名列）。title 仅回退用
    let base = format!(
        "SELECT id, directory, title, parent_id, time_created, time_updated FROM {}",
        session_table(conn)
    );
    let sql = match id {
        Some(_) => format!("{base} WHERE id = ?1"),
        None => format!("{base} WHERE parent_id IS NULL"),
    };
    let mut stmt = conn.prepare(&sql)?;
    let map = |r: &rusqlite::Row| {
        Ok(OcRow {
            id: r.get(0)?,
            directory: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
            title: r.get(2)?,
            parent_id: r.get(3)?,
            created_ms: r.get::<_, Option<i64>>(4)?.unwrap_or(0),
            updated_ms: r.get::<_, Option<i64>>(5)?.unwrap_or(0),
        })
    };
    let rows = match id {
        Some(i) => stmt.query_map([i], map)?.collect::<rusqlite::Result<Vec<_>>>()?,
        None => stmt.query_map([], map)?.collect::<rusqlite::Result<Vec<_>>>()?,
    };
    Ok(rows)
}

// ── 消息构造 ───────────────────────────────────────────────────────

fn ms_to_rfc3339(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.to_rfc3339())
        .unwrap_or_default()
}

fn oc_msg(uuid: &str, ty: MessageType, role: &str, ts_ms: i64, cwd: &str) -> ParsedMessage {
    ParsedMessage {
        uuid: uuid.to_string(),
        parent_uuid: None,
        message_type: ty,
        timestamp: ms_to_rfc3339(ts_ms),
        role: Some(role.to_string()),
        content: Value::Null,
        usage: None,
        model: None,
        cwd: Some(cwd.to_string()),
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

/// 累计 usage（step-finish 挂 assistant，多 step 相加）。
fn add_usage(msg: &mut ParsedMessage, tokens: &Value) {
    let get = |k: &str| tokens.get(k).and_then(|v| v.as_i64()).unwrap_or(0);
    let cache = |k: &str| {
        tokens
            .pointer(&format!("/cache/{k}"))
            .and_then(|v| v.as_i64())
    };
    let usage = msg.usage.take().unwrap_or(UsageMetadata {
        input_tokens: 0,
        output_tokens: 0,
        cache_read_input_tokens: None,
        cache_creation_input_tokens: None,
    });
    msg.usage = Some(UsageMetadata {
        input_tokens: usage.input_tokens + get("input") as u64,
        output_tokens: usage.output_tokens + get("output") as u64,
        cache_read_input_tokens: match (usage.cache_read_input_tokens, cache("read")) {
            (Some(a), Some(b)) => Some(a + b as u64),
            (a, b) => a.or(b.map(|v| v as u64)),
        },
        cache_creation_input_tokens: match (usage.cache_creation_input_tokens, cache("write")) {
            (Some(a), Some(b)) => Some(a + b as u64),
            (a, b) => a.or(b.map(|v| v as u64)),
        },
    });
}

/// v1 正文：message（角色/时间）+ part（内容块）按 message_id 分组。
fn parse_v1_messages(conn: &Connection, sid: &str, cwd: &str) -> (Vec<ParsedMessage>, u32) {
    let mut parts_by_msg: std::collections::HashMap<String, Vec<Value>> =
        std::collections::HashMap::new();
    {
        let mut stmt = match conn.prepare(
            "SELECT message_id, data FROM part WHERE session_id = ?1 ORDER BY message_id, id",
        ) {
            Ok(s) => s,
            Err(_) => return (Vec::new(), 0),
        };
        let rows = match stmt.query_map([sid], |p| {
            Ok((p.get::<_, String>(0)?, p.get::<_, String>(1)?))
        }) {
            Ok(r) => r,
            Err(_) => return (Vec::new(), 0),
        };
        for (mid, data) in rows.flatten() {
            if let Ok(v) = serde_json::from_str::<Value>(&data) {
                parts_by_msg.entry(mid).or_default().push(v);
            }
        }
    }

    let mut messages: Vec<ParsedMessage> = Vec::new();
    let mut unknown = 0u32;
    let mut model: Option<String> = None;
    let mut stmt = match conn
        .prepare("SELECT id, data FROM message WHERE session_id = ?1 ORDER BY time_created, id")
    {
        Ok(s) => s,
        Err(_) => return (Vec::new(), 0),
    };
    let msg_rows = match stmt.query_map([sid], |m| {
        Ok((m.get::<_, String>(0)?, m.get::<_, String>(1)?))
    }) {
        Ok(r) => r.flatten().collect::<Vec<_>>(),
        Err(_) => return (Vec::new(), 0),
    };

    for (mid, data) in msg_rows {
        let Ok(md) = serde_json::from_str::<Value>(&data) else {
            unknown += 1;
            continue;
        };
        let ts = md
            .pointer("/time/created")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let role = md.get("role").and_then(|v| v.as_str()).unwrap_or("");
        // model 记录在 assistant message data（session 表无此列）
        if role == "assistant" {
            if let Some(m) = md.get("modelID").and_then(|v| v.as_str()) {
                model = Some(m.to_string());
            }
        }

        let parts = parts_by_msg.remove(&mid).unwrap_or_default();
        match role {
            "user" => {
                // user：parts 中的 text；synthetic 标记注入内容。真人文本与
                // 注入可同存于一条 message（实测：用户输入 + 文件树注入）——
                // is_meta 仅在**纯注入**（无真人文本）时为 true；synthetic
                // 文本一律不进正文
                let mut texts: Vec<String> = Vec::new();
                let mut has_synthetic = false;
                for p in &parts {
                    match p.get("type").and_then(|v| v.as_str()) {
                        Some("text") => {
                            let t = p.get("text").and_then(|v| v.as_str()).unwrap_or("").trim();
                            if t.is_empty() {
                                continue;
                            }
                            if p.get("synthetic").and_then(|v| v.as_bool()) == Some(true) {
                                has_synthetic = true;
                                continue;
                            }
                            texts.push(t.to_string());
                        }
                        _ => {}
                    }
                }
                if texts.is_empty() && !has_synthetic {
                    continue;
                }
                let mut m = oc_msg(&mid, MessageType::User, "user", ts, cwd);
                m.is_meta = texts.is_empty() && has_synthetic;
                m.content = Value::Array(
                    texts
                        .iter()
                        .map(|t| serde_json::json!({"type": "text", "text": t}))
                        .collect(),
                );
                messages.push(m);
            }
            "assistant" => {
                // assistant：text→text 块；reasoning→thinking；tool→tool_use
                let mut blocks: Vec<Value> = Vec::new();
                let mut tool_calls = vec![];
                for p in &parts {
                    match p.get("type").and_then(|v| v.as_str()) {
                        Some("text") => {
                            let t = p.get("text").and_then(|v| v.as_str()).unwrap_or("").trim();
                            if !t.is_empty() {
                                blocks.push(serde_json::json!({"type": "text", "text": t}));
                            }
                        }
                        Some("reasoning") => {
                            let t = p.get("text").and_then(|v| v.as_str()).unwrap_or("");
                            if !t.trim().is_empty() {
                                blocks.push(serde_json::json!({
                                    "type": "thinking", "thinking": t, "signature": ""
                                }));
                            }
                        }
                        Some("tool") => {
                            let call_id = p.get("callID").and_then(|v| v.as_str()).unwrap_or("");
                            let name = p.get("tool").and_then(|v| v.as_str()).unwrap_or("tool");
                            let input = p.pointer("/state/input").cloned().unwrap_or(Value::Null);
                            blocks.push(serde_json::json!({
                                "type": "tool_use", "id": call_id, "name": name, "input": input
                            }));
                            tool_calls.push(crate::types::messages::ToolCall {
                                id: call_id.to_string(),
                                name: name.to_string(),
                                input,
                                is_task: false,
                                task_description: None,
                                task_subagent_type: None,
                            });
                        }
                        Some("step-finish") => {
                            if let Some(tokens) = p.get("tokens") {
                                // 先落 message 再累加（usage 挂在 message 上）
                                let _ = tokens;
                            }
                        }
                        // step-start/snapshot/patch/file：噪声
                        _ => {}
                    }
                }
                if blocks.is_empty() {
                    continue;
                }
                let mut m = oc_msg(&mid, MessageType::Assistant, "assistant", ts, cwd);
                m.model = model.clone();
                m.tool_calls = tool_calls;
                m.content = Value::Array(blocks);
                messages.push(m);
            }
            _ => unknown += 1,
        }

        // tool 输出 → 紧随的 user 载体（is_meta + tool_result），并在此处理
        // step-finish usage（归属当前 assistant —— OpenCode v1 里 part 挂在
        // assistant message 上，含 step-finish）
        if role == "assistant" {
            let mut result_blocks: Vec<Value> = Vec::new();
            let mut usage_tokens: Option<&Value> = None;
            for p in &parts {
                match p.get("type").and_then(|v| v.as_str()) {
                    Some("tool") => {
                        let call_id = p.get("callID").and_then(|v| v.as_str()).unwrap_or("");
                        let output = p
                            .pointer("/state/output")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let status_err = p.pointer("/state/status").and_then(|v| v.as_str())
                            == Some("error");
                        if !call_id.is_empty() {
                            result_blocks.push(serde_json::json!({
                                "type": "tool_result",
                                "tool_use_id": call_id,
                                "content": output,
                                "is_error": status_err,
                            }));
                        }
                    }
                    Some("step-finish") => {
                        if let Some(t) = p.get("tokens") {
                            usage_tokens = Some(t);
                        }
                    }
                    _ => {}
                }
            }
            if let Some(msg) = messages.last_mut() {
                if let Some(tokens) = usage_tokens {
                    add_usage(msg, tokens);
                }
            }
            if !result_blocks.is_empty() {
                let mut carrier = oc_msg(
                    &format!("{mid}-results"),
                    MessageType::User,
                    "user",
                    ts,
                    cwd,
                );
                carrier.is_meta = true;
                carrier.content = Value::Array(result_blocks);
                carrier.tool_results = crate::parsing::extract_tool_results(&carrier.content);
                messages.push(carrier);
            }
        }
    }
    (messages, unknown)
}

/// v2 正文：session_message 单表按 seq 有序（type 列 user/synthetic/
/// assistant/shell + data JSON）。本机无 v2 数据，按 Wake 实测格式实现。
fn parse_v2_messages(conn: &Connection, sid: &str, cwd: &str) -> (Vec<ParsedMessage>, u32) {
    let mut messages: Vec<ParsedMessage> = Vec::new();
    let mut unknown = 0u32;
    let Ok(mut stmt) = conn.prepare(
        "SELECT id, type, data FROM session_message WHERE session_id = ?1 ORDER BY seq",
    ) else {
        return (Vec::new(), 0);
    };
    let Ok(rows) = stmt.query_map([sid], |m| {
        Ok((
            m.get::<_, String>(0)?,
            m.get::<_, String>(1)?,
            m.get::<_, String>(2)?,
        ))
    }) else {
        return (Vec::new(), 0);
    };

    for (mid, mtype, data) in rows.flatten() {
        let Ok(md) = serde_json::from_str::<Value>(&data) else {
            unknown += 1;
            continue;
        };
        let ts = md
            .pointer("/time/created")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        match mtype.as_str() {
            "user" => {
                let text = md.get("text").and_then(|v| v.as_str()).unwrap_or("").trim();
                if !text.is_empty() {
                    let mut m = oc_msg(&mid, MessageType::User, "user", ts, cwd);
                    m.content = Value::Array(vec![serde_json::json!({"type":"text","text":text})]);
                    messages.push(m);
                }
            }
            "synthetic" | "system" => {
                // 注入内容（编辑器上下文等）→ is_meta 折叠
                let text = md.get("text").and_then(|v| v.as_str()).unwrap_or("").trim();
                if !text.is_empty() {
                    let mut m = oc_msg(&mid, MessageType::User, "user", ts, cwd);
                    m.is_meta = true;
                    m.content = Value::Array(vec![serde_json::json!({"type":"text","text":text})]);
                    messages.push(m);
                }
            }
            "assistant" => {
                let mut blocks: Vec<Value> = Vec::new();
                let mut tool_calls = vec![];
                for b in md
                    .get("content")
                    .and_then(|c| c.as_array())
                    .into_iter()
                    .flatten()
                {
                    match b.get("type").and_then(|v| v.as_str()) {
                        Some("text") => {
                            let t = b.get("text").and_then(|v| v.as_str()).unwrap_or("").trim();
                            if !t.is_empty() {
                                blocks.push(serde_json::json!({"type":"text","text":t}));
                            }
                        }
                        Some("reasoning") => {
                            let t = b.get("text").and_then(|v| v.as_str()).unwrap_or("");
                            if !t.trim().is_empty() {
                                blocks.push(serde_json::json!({
                                    "type":"thinking","thinking":t,"signature":""
                                }));
                            }
                        }
                        Some("tool") => {
                            let call_id = b.get("callID").and_then(|v| v.as_str()).unwrap_or("");
                            let name = b.get("tool").and_then(|v| v.as_str()).unwrap_or("tool");
                            let input = b.pointer("/state/input").cloned().unwrap_or(Value::Null);
                            blocks.push(serde_json::json!({
                                "type":"tool_use","id":call_id,"name":name,"input":input
                            }));
                            tool_calls.push(crate::types::messages::ToolCall {
                                id: call_id.to_string(),
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
                if blocks.is_empty() {
                    continue;
                }
                let mut m = oc_msg(&mid, MessageType::Assistant, "assistant", ts, cwd);
                m.model = md
                    .pointer("/model/id")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                m.tool_calls = tool_calls;
                m.content = Value::Array(blocks);
                // usage：v2 在 message 顶层 tokens
                if let Some(tokens) = md.get("tokens") {
                    add_usage(&mut m, tokens);
                }
                messages.push(m);
                // tool 输出 → user 载体
                let result_blocks: Vec<Value> = md
                    .get("content")
                    .and_then(|c| c.as_array())
                    .into_iter()
                    .flatten()
                    .filter(|b| b.get("type").and_then(|v| v.as_str()) == Some("tool"))
                    .filter_map(|b| {
                        let call_id = b.get("callID").and_then(|v| v.as_str())?;
                        if call_id.is_empty() {
                            return None;
                        }
                        Some(serde_json::json!({
                            "type": "tool_result",
                            "tool_use_id": call_id,
                            "content": b.pointer("/state/output").and_then(|v| v.as_str()).unwrap_or(""),
                            "is_error": b.pointer("/state/status").and_then(|v| v.as_str()) == Some("error"),
                        }))
                    })
                    .collect();
                if !result_blocks.is_empty() {
                    let mut carrier =
                        oc_msg(&format!("{mid}-results"), MessageType::User, "user", ts, cwd);
                    carrier.is_meta = true;
                    carrier.content = Value::Array(result_blocks);
                    carrier.tool_results = crate::parsing::extract_tool_results(&carrier.content);
                    messages.push(carrier);
                }
            }
            "shell" => {
                let command = md.get("command").and_then(|v| v.as_str()).unwrap_or("");
                let input = serde_json::json!({"command": command});
                let output = md
                    .get("output")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let mut m = oc_msg(&mid, MessageType::Assistant, "assistant", ts, cwd);
                let block = serde_json::json!({
                    "type": "tool_use",
                    "id": md.get("callID").and_then(|v| v.as_str()).unwrap_or(""),
                    "name": "shell",
                    "input": input,
                });
                m.tool_calls = vec![crate::types::messages::ToolCall {
                    id: md.get("callID").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    name: "shell".to_string(),
                    input: serde_json::json!({"command": command}),
                    is_task: false,
                    task_description: None,
                    task_subagent_type: None,
                }];
                m.content = Value::Array(vec![block]);
                messages.push(m);
                let mut carrier = oc_msg(&format!("{mid}-results"), MessageType::User, "user", ts, cwd);
                carrier.is_meta = true;
                let rb = serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": md.get("callID").and_then(|v| v.as_str()).unwrap_or(""),
                    "content": output,
                    "is_error": false,
                });
                carrier.content = Value::Array(vec![rb]);
                carrier.tool_results = crate::parsing::extract_tool_results(&carrier.content);
                messages.push(carrier);
            }
            _ => unknown += 1,
        }
    }
    (messages, unknown)
}

/// light preview：首条真人 user 文本 + 计数（user+配对 assistant，对齐
/// claude 口径）。SQL 查询，零文件读 —— SQLite 型的优势。
struct OcPreview {
    first_message: Option<String>,
    message_count: u32,
}

fn query_preview(conn: &Connection, sid: &str) -> OcPreview {
    let mut preview = OcPreview {
        first_message: None,
        message_count: 0,
    };
    if uses_v2(conn) {
        let Ok(mut stmt) = conn.prepare(
            "SELECT type, data FROM session_message WHERE session_id = ?1 ORDER BY seq",
        ) else {
            return preview;
        };
        let Ok(rows) = stmt.query_map([sid], |m| {
            Ok((m.get::<_, String>(0)?, m.get::<_, String>(1)?))
        }) else {
            return preview;
        };
        let mut awaiting_ai = false;
        for (mtype, data) in rows.flatten() {
            let md = serde_json::from_str::<Value>(&data).ok();
            match mtype.as_str() {
                "user" | "synthetic" => {
                    preview.message_count += 1;
                    awaiting_ai = true;
                    let text = md
                        .as_ref()
                        .and_then(|d| d.get("text"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if preview.first_message.is_none() && mtype == "user" && !text.trim().is_empty()
                    {
                        preview.first_message =
                            Some(text.trim().chars().take(100).collect());
                    }
                }
                "assistant" if awaiting_ai => {
                    preview.message_count += 1;
                    awaiting_ai = false;
                }
                "shell" => {
                    preview.message_count += 1;
                    awaiting_ai = true;
                }
                _ => {}
            }
        }
        return preview;
    }

    // v1：user/synthetic text part 与 assistant message 计数
    let mut awaiting_ai = false;
    let Ok(mut stmt) =
        conn.prepare("SELECT data FROM message WHERE session_id = ?1 ORDER BY time_created, id")
    else {
        return preview;
    };
    let Ok(rows) = stmt.query_map([sid], |m| Ok(m.get::<_, String>(0)?)) else {
        return preview;
    };
    let msgs: Vec<String> = rows.flatten().collect();
    for data in &msgs {
        let Ok(md) = serde_json::from_str::<Value>(data) else { continue };
        match md.get("role").and_then(|v| v.as_str()) {
            Some("user") => {
                preview.message_count += 1;
                awaiting_ai = true;
                if preview.first_message.is_none() {
                    // 首条真人文本：查该 message 的 parts（synthetic 排除）
                    let Ok(mid) = serde_json::from_str::<Value>(data) else { continue };
                    let _ = mid;
                    // message id 不在 data 里 —— 由调用方按序查询 parts。
                    // 这里退化为 title 回退（见 light_session）
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

/// v1 首条真人 user 文本（按 message 顺序查 parts，synthetic 排除）。
fn query_first_user_text(conn: &Connection, sid: &str) -> Option<String> {
    let mut stmt = conn.prepare(
        "SELECT m.id FROM message m WHERE m.session_id = ?1 AND json_extract(m.data, '$.role') = 'user' ORDER BY m.time_created LIMIT 1",
    )
    .ok()?;
    let mid: String = stmt.query_row([sid], |r| r.get(0)).ok()?;
    let mut p = conn.prepare(
        "SELECT data FROM part WHERE message_id = ?1 AND json_extract(data, '$.type') = 'text' AND COALESCE(json_extract(data, '$.synthetic'), 0) != 1 ORDER BY id LIMIT 1",
    )
    .ok()?;
    let text: String = p
        .query_row([mid], |r| r.get(0))
        .ok()
        .and_then(|d: String| serde_json::from_str::<Value>(&d).ok())
        .and_then(|v| v.get("text").and_then(|t| t.as_str()).map(String::from))?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.chars().take(100).collect())
}

impl AgentAdapter for OpencodeAdapter {
    fn kind(&self) -> AgentKind {
        AgentKind::Opencode
    }

    fn data_roots(&self) -> Vec<PathBuf> {
        self.db_dirs(&dirs::home_dir().unwrap_or_default())
    }

    fn owns_path(&self, path: &Path) -> bool {
        // 结构特征：路径任一组件是 opencode 库文件名（含虚拟路径形态 ——
        // `#` 后缀不影响组件匹配，因为 `#` 只出现在文件名尾部）
        let comps: Vec<&str> = path
            .components()
            .filter_map(|c| c.as_os_str().to_str())
            .collect();
        comps
            .iter()
            .any(|c| DB_NAMES.iter().any(|n| c.starts_with(n)))
    }

    fn data_root_under(&self, home: &Path) -> PathBuf {
        home.join(".local").join("share").join("opencode")
    }

    fn watch_roots_under(&self, _home: &Path) -> Vec<PathBuf> {
        // SQLite 型无实时刷新（既定决策）：watcher 只认 .jsonl，库文件
        // mtime 不触发事件；监听库目录只会在 SSH 轮询里空转 readdir。
        // 列表靠 120s TTL 兑底刷新
        Vec::new()
    }

    fn scan_sessions(&self, root: &Path, fs: &dyn FsProvider) -> Vec<AgentSessionEntry> {
        if !self.ssh_guard(fs) {
            return Vec::new();
        }
        let mut entries = Vec::new();
        // env 额外目录也扫（本地上下文生效）
        let mut dirs = vec![root.to_path_buf()];
        for d in &self.extra_dirs {
            if !dirs.contains(d) {
                dirs.push(d.clone());
            }
        }
        for dir in dirs {
            for name in DB_NAMES {
                let db = dir.join(name);
                let Ok(meta) = std::fs::metadata(&db) else {
                    continue;
                };
                if !meta.is_file() {
                    continue;
                }
                let Some(conn) = open_ro(&db) else {
                    log::warn!("agents: cannot open opencode db readonly: {}", db.display());
                    continue;
                };
                let mtime_ms = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                match query_rows(&conn, None) {
                    Ok(rows) => {
                        for r in rows {
                            if r.directory.is_empty() {
                                continue;
                            }
                            entries.push(AgentSessionEntry {
                                agent: AgentKind::Opencode,
                                project_id: encode_path(&r.directory),
                                project_path: r.directory.clone(),
                                session_id: r.id.clone(),
                                file_path: virtual_path(&db, &r.id),
                                mtime_ms,
                                birthtime_ms: r.created_ms.max(0) as u64,
                                created_ms: r.created_ms.max(0) as u64,
                            });
                        }
                    }
                    Err(e) => {
                        log::warn!("agents: opencode query failed on {}: {e}", db.display());
                    }
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
        if !self.ssh_guard(fs) {
            return None;
        }
        let mut dirs = vec![root.to_path_buf()];
        for d in &self.extra_dirs {
            if !dirs.contains(d) {
                dirs.push(d.clone());
            }
        }
        for dir in dirs {
            for name in DB_NAMES {
                let db = dir.join(name);
                if !db.is_file() {
                    continue;
                }
                let conn = open_ro(&db)?;
                if let Ok(rows) = query_rows(&conn, Some(session_id)) {
                    if let Some(r) = rows.first() {
                        if id_matches(&r.directory) {
                            return Some(virtual_path(&db, session_id));
                        }
                    }
                }
            }
        }
        None
    }

    fn parse_messages(&self, path: &Path, fs: &dyn FsProvider) -> Vec<ParsedMessage> {
        if !self.ssh_guard(fs) {
            return vec![];
        }
        let Some((db, sid)) = parse_virtual_path(path) else {
            return vec![];
        };
        let Some(conn) = open_ro(&db) else {
            return vec![];
        };
        let Some(row) = query_rows(&conn, Some(&sid)).ok().and_then(|r| r.into_iter().next())
        else {
            return vec![];
        };
        let cwd = row.directory;
        let (messages, unknown) = if uses_v2(&conn) {
            parse_v2_messages(&conn, &sid, &cwd)
        } else {
            parse_v1_messages(&conn, &sid, &cwd)
        };
        if unknown > 0 {
            log::debug!("opencode: {} unknown rows in {}", unknown, sid);
        }
        messages
    }

    fn light_session(&self, entry: &AgentSessionEntry, fs: &dyn FsProvider) -> Option<Session> {
        if !self.ssh_guard(fs) {
            return None;
        }
        let (db, sid) = parse_virtual_path(&entry.file_path)?;
        let conn = open_ro(&db)?;
        let row = query_rows(&conn, Some(&sid)).ok()?.into_iter().next()?;
        let preview = query_preview(&conn, &sid);
        let first_message = preview
            .first_message
            .or_else(|| query_first_user_text(&conn, &sid))
            .or_else(|| row.title.as_deref().map(|t| t.chars().take(100).collect()));
        Some(Session {
            id: entry.session_id.clone(),
            agent: AgentKind::Opencode,
            project_id: entry.project_id.clone(),
            project_path: entry.project_path.clone(),
            created_at: entry.created_ms.max(0) as u64,
            updated_at: Some(row.updated_ms.max(0) as u64),
            todo_data: None,
            first_message,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::AgentAdapter;
    use crate::infrastructure::fs_provider::LocalFsProvider;
    use std::sync::Arc;

    /// 构造 v1 fixture DB（schema 对齐本机实测）。
    fn make_v1_db(dir: &Path) -> PathBuf {
        let db = dir.join("opencode.db");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE session (
                id TEXT PRIMARY KEY, project_id TEXT NOT NULL, parent_id TEXT,
                slug TEXT NOT NULL, directory TEXT NOT NULL, title TEXT NOT NULL,
                version TEXT NOT NULL, share_url TEXT,
                time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL
            );
            CREATE TABLE message (
                id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );
            CREATE TABLE part (
                id TEXT PRIMARY KEY, message_id TEXT NOT NULL, session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );
            INSERT INTO session VALUES ('ses_main','p1',NULL,'s','/Users/x/proj','Background: undefined','1.0',NULL,1773741712000,1773741765000);
            INSERT INTO session VALUES ('ses_child','p1','ses_main','s','/Users/x/proj','','1.0',NULL,1,2);
            INSERT INTO message VALUES ('msg_u1','ses_main',1,1,'{"role":"user","time":{"created":1773741712318}}');
            INSERT INTO message VALUES ('msg_a1','ses_main',2,2,'{"role":"assistant","time":{"created":1773741712323},"modelID":"glm-4.7"}');
            INSERT INTO part VALUES ('p1','msg_u1','ses_main',1,1,'{"type":"text","text":"hi"}');
            INSERT INTO part VALUES ('p2','msg_u1','ses_main',1,1,'{"type":"text","text":"/file/tree...","synthetic":true}');
            INSERT INTO part VALUES ('p3','msg_a1','ses_main',2,2,'{"type":"step-start","snapshot":"x"}');
            INSERT INTO part VALUES ('p4','msg_a1','ses_main',2,2,'{"type":"reasoning","text":"thinking..."}');
            INSERT INTO part VALUES ('p5','msg_a1','ses_main',2,2,'{"type":"text","text":"Hello!"}');
            INSERT INTO part VALUES ('p6','msg_a1','ses_main',2,2,'{"type":"tool","callID":"call_1","tool":"bash","state":{"status":"completed","input":{"command":"ls"},"output":"file1"}}');
            INSERT INTO part VALUES ('p7','msg_a1','ses_main',2,2,'{"type":"step-finish","reason":"stop","tokens":{"total":30119,"input":29508,"output":135,"reasoning":118,"cache":{"read":476,"write":7}}}');
            "#,
        )
        .unwrap();
        drop(conn);
        db
    }

    #[test]
    fn scan_filters_subagent_and_addresses_via_virtual_path() {
        let dir = tempfile::tempdir().unwrap();
        let db = make_v1_db(dir.path());
        let fs = Arc::new(LocalFsProvider::new());
        let adapter = OpencodeAdapter::new();
        let entries = adapter.scan_sessions(dir.path(), fs.as_ref());

        // 子代理（parent_id 非空）被过滤，只留主会话
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.session_id, "ses_main");
        assert_eq!(e.project_path, "/Users/x/proj");
        assert_eq!(e.project_id, "-Users-x-proj");
        assert_eq!(e.file_path, virtual_path(&db, "ses_main"), "虚拟路径寻址");

        // locate 归属校验
        let ok = |cwd: &str| cwd == "/Users/x/proj";
        assert_eq!(
            adapter.locate_session(dir.path(), "ses_main", fs.as_ref(), &ok),
            Some(virtual_path(&db, "ses_main"))
        );
    }

    #[test]
    fn parse_maps_v1_to_claude_semantics() {
        let dir = tempfile::tempdir().unwrap();
        let db = make_v1_db(dir.path());
        let fs = Arc::new(LocalFsProvider::new());
        let adapter = OpencodeAdapter::new();
        let vpath = virtual_path(&db, "ses_main");
        let msgs = adapter.parse_messages(&vpath, fs.as_ref());

        // user（synthetic 注入独立成 meta 载体）+ assistant + tool_result 载体
        assert_eq!(msgs.len(), 3, "user / assistant / results-carrier");

        let u = &msgs[0];
        assert_eq!(u.uuid, "msg_u1");
        assert_eq!(u.message_type, MessageType::User);
        assert!(!u.is_meta, "有真人文本的 user 不是纯 meta");
        let blocks = u.content.as_array().unwrap();
        assert_eq!(blocks.len(), 1, "synthetic 文本不进正文");

        let a = &msgs[1];
        assert_eq!(a.model.as_deref(), Some("glm-4.7"));
        assert_eq!(a.tool_calls.len(), 1);
        assert_eq!(a.tool_calls[0].name, "bash");
        assert_eq!(a.tool_calls[0].input, serde_json::json!({"command": "ls"}));
        // usage 来自 step-finish（含 cache）
        let usage = a.usage.as_ref().unwrap();
        assert_eq!(usage.input_tokens, 29508);
        assert_eq!(usage.output_tokens, 135);
        assert_eq!(usage.cache_read_input_tokens, Some(476));
        assert_eq!(usage.cache_creation_input_tokens, Some(7));
        // blocks: reasoning + text + tool_use（step-start 不产出）
        let ab = a.content.as_array().unwrap();
        assert_eq!(ab.len(), 3);
        assert_eq!(ab[0].get("type"), Some(&serde_json::json!("thinking")));

        let r = &msgs[2];
        assert!(r.is_meta);
        assert_eq!(r.tool_results.len(), 1);
        assert_eq!(r.tool_results[0].tool_use_id, "call_1");
        assert_eq!(r.tool_results[0].content, serde_json::json!("file1"));
    }

    #[test]
    fn light_session_uses_first_real_user_text() {
        let dir = tempfile::tempdir().unwrap();
        let db = make_v1_db(dir.path());
        let fs = Arc::new(LocalFsProvider::new());
        let adapter = OpencodeAdapter::new();
        let entries = adapter.scan_sessions(dir.path(), fs.as_ref());
        let light = adapter.light_session(&entries[0], fs.as_ref()).unwrap();

        assert_eq!(light.agent, AgentKind::Opencode);
        assert_eq!(
            light.first_message.as_deref(),
            Some("hi"),
            "真人文本优先于 synthetic 与垃圾 title"
        );
        assert!(light.message_count >= 2, "user + 配对 assistant");
        assert_eq!(light.updated_at, Some(1773741765000));
    }

    #[test]
    fn owns_path_matches_db_names() {
        assert!(OpencodeAdapter::new().owns_path(Path::new(
            "/home/u/.local/share/opencode/opencode.db#ses_x"
        )));
        assert!(OpencodeAdapter::new().owns_path(Path::new(
            "/home/u/.local/share/opencode/opencode-next.db"
        )));
        assert!(!OpencodeAdapter::new().owns_path(Path::new(
            "/home/u/.pi/agent/sessions/--x--/a.jsonl"
        )));
    }

    /// 真实数据 smoke（本机装有 opencode 时）：`cargo test -- --ignored`
    #[test]
    #[ignore = "依赖本机 ~/.local/share/opencode 真实数据"]
    fn real_data_smoke() {
        let home = dirs::home_dir().unwrap();
        let root = OpencodeAdapter::new().data_root_under(&home);
        if !root.is_dir() {
            eprintln!("no real opencode data, skip");
            return;
        }
        let fs = Arc::new(LocalFsProvider::new());
        let adapter = OpencodeAdapter::new();
        let entries = adapter.scan_sessions(&root, fs.as_ref());
        eprintln!("scanned {} opencode sessions", entries.len());
        assert!(!entries.is_empty(), "本机应有 opencode 数据");

        for e in entries.iter().take(20) {
            let light = adapter
                .light_session(e, fs.as_ref())
                .unwrap_or_else(|| panic!("light failed: {}", e.session_id));
            let msgs = adapter.parse_messages(&e.file_path, fs.as_ref());
            let parsed = crate::parsing::process_messages(&msgs);
            let chunks = crate::analysis::ChunkBuilder::build_chunks(&msgs, &[]);
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

