//! Claude Code 适配器 —— 收编既有的 JSONL 解析管线（行为不变基线）。
//!
//! 数据源：`~/.claude/projects/{cwd 编码目录}/*.jsonl`（Anthropic 块协议
//! 的 JSONL wire format，见 `types/jsonl.rs`）。
//!
//! 相对旧管线（`parse_session_file_with_provider`）的唯一语义差异：
//! **requestId 去重收编到 adapter 内部**。旧管线在 metrics/search 路径
//! 去重而展示路径不去重；契约要求「adapter 输出即终态」，故在输出前统一
//! 去重（去重是幂等操作，下游 `calculate_metrics` / `session_searcher`
//! 的二次去重保持不变）。本机真实数据无 requestId 字段，零行为差异。

use std::path::{Path, PathBuf};

use crate::agents::AgentAdapter;
use crate::infrastructure::fs_provider::FsProvider;
use crate::parsing::{deduplicate_by_request_id, parse_jsonl_content};
use crate::types::domain::AgentKind;
use crate::types::messages::ParsedMessage;

pub struct ClaudeAdapter {
    projects_dir: PathBuf,
}

impl ClaudeAdapter {
    pub fn new() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        Self {
            projects_dir: home.join(".claude").join("projects"),
        }
    }
}

impl Default for ClaudeAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentAdapter for ClaudeAdapter {
    fn kind(&self) -> AgentKind {
        AgentKind::ClaudeCode
    }

    fn data_roots(&self) -> Vec<PathBuf> {
        vec![self.projects_dir.clone()]
    }

    fn parse_messages(&self, path: &Path, fs: &dyn FsProvider) -> Vec<ParsedMessage> {
        let content = match fs.read_file(path) {
            Ok(c) => c,
            Err(_) => return vec![],
        };
        let parsed = parse_jsonl_content(&content);
        deduplicate_by_request_id(&parsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::fs_provider::LocalFsProvider;
    use std::sync::Arc;

    /// 无 requestId 的数据（本机真实形态）：adapter 输出与旧管线逐条一致。
    #[test]
    fn output_matches_legacy_pipeline_without_request_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s1.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"type\":\"user\",\"uuid\":\"u1\",\"message\":{\"role\":\"user\",\"content\":\"hi\"},\"timestamp\":\"2026-01-01T00:00:00Z\"}\n",
                "{\"type\":\"assistant\",\"uuid\":\"a1\",\"message\":{\"role\":\"assistant\",\"id\":\"m1\",\"type\":\"message\",\"model\":\"claude\",\"content\":[{\"type\":\"text\",\"text\":\"hello\"}],\"usage\":{\"input_tokens\":10,\"output_tokens\":5}},\"timestamp\":\"2026-01-01T00:00:01Z\"}\n",
            ),
        )
        .unwrap();

        let fs = Arc::new(LocalFsProvider::new());
        let adapter = ClaudeAdapter::new();
        let got = adapter.parse_messages(&path, fs.as_ref());

        let legacy = crate::parsing::parse_jsonl_content(&std::fs::read_to_string(&path).unwrap());
        assert_eq!(got.len(), legacy.len());
        assert_eq!(got[0].uuid, "u1");
        assert_eq!(got[1].uuid, "a1");
        assert_eq!(got[1].usage.as_ref().unwrap().input_tokens, 10);
    }

    /// 去重契约：同 requestId 的流式重复行只保留最后一条（adapter 输出即终态）。
    #[test]
    fn duplicate_request_id_lines_are_deduplicated_in_adapter() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s2.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"type\":\"assistant\",\"uuid\":\"a1\",\"requestId\":\"r1\",\"message\":{\"role\":\"assistant\",\"id\":\"m1\",\"type\":\"message\",\"model\":\"claude\",\"content\":[{\"type\":\"text\",\"text\":\"partial\"}],\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n",
                "{\"type\":\"assistant\",\"uuid\":\"a2\",\"requestId\":\"r1\",\"message\":{\"role\":\"assistant\",\"id\":\"m1\",\"type\":\"message\",\"model\":\"claude\",\"content\":[{\"type\":\"text\",\"text\":\"final\"}],\"usage\":{\"input_tokens\":2,\"output_tokens\":2}}}\n",
                "{\"type\":\"assistant\",\"uuid\":\"a3\",\"requestId\":\"r2\",\"message\":{\"role\":\"assistant\",\"id\":\"m2\",\"type\":\"message\",\"model\":\"claude\",\"content\":[{\"type\":\"text\",\"text\":\"other\"}],\"usage\":{\"input_tokens\":4,\"output_tokens\":4}}}\n",
            ),
        )
        .unwrap();

        let fs = Arc::new(LocalFsProvider::new());
        let got = ClaudeAdapter::new().parse_messages(&path, fs.as_ref());

        assert_eq!(got.len(), 2);
        assert_eq!(got[0].uuid, "a2", "keeps last line of r1");
        assert_eq!(got[1].uuid, "a3");
    }

    /// 文件不存在 → 空输出（与旧管线容错语义一致）。
    #[test]
    fn missing_file_returns_empty() {
        let fs = Arc::new(LocalFsProvider::new());
        let got = ClaudeAdapter::new().parse_messages(Path::new("/nonexistent/s.jsonl"), fs.as_ref());
        assert!(got.is_empty());
    }

    #[test]
    fn data_root_is_claude_projects_dir() {
        let roots = ClaudeAdapter::new().data_roots();
        assert_eq!(roots.len(), 1);
        assert!(roots[0].ends_with(".claude/projects"));
    }
}
