//! 路径解析 — 从文件路径分段中提取 projectId、sessionId、isSubagent。

use super::FileWatcher;

impl FileWatcher {
    /// 解析路径分段，提取 projectId、sessionId 和 isSubagent。
    ///
    /// 与 Electron FileWatcher.ts 逻辑一致（第 507-533 行）:
    /// - 会话文件（2 段）: `projectId/sessionId.jsonl`
    /// - 子代理文件（4 段）: `projectId/sessionId/subagents/agent-hash.jsonl`
    pub(crate) fn parse_path_parts(parts: &[&str]) -> (Option<String>, Option<String>, bool) {
        if parts.is_empty() {
            return (None, None, false);
        }

        let project_id = Some(parts[0].to_string());

        // 项目根目录下的会话文件: projectId/sessionId.jsonl
        if parts.len() == 2 && parts[1].ends_with(".jsonl") {
            let session_id = parts[1].strip_suffix(".jsonl").map(|s| s.to_string());
            return (project_id, session_id, false);
        }

        // 子代理文件: projectId/sessionId/subagents/agent-hash.jsonl
        if parts.len() == 4 && parts[2] == "subagents" && parts[3].ends_with(".jsonl") {
            let session_id = parts[1].to_string();
            return (project_id, Some(session_id), true);
        }

        (project_id, None, false)
    }
}
