//! 路径规范化工具函数。

/// 规范化 Claude Root Path（与 Electron 的 normalizeConfiguredClaudeRootPath 对齐）。
///
/// 执行以下处理：
/// 1. 解析 `.` 和 `..` 路径段
/// 2. 折叠连续分隔符
/// 3. 去除尾部斜杠（保留根路径 `/`）
pub(crate) fn normalize_claude_root_path(path: &str) -> String {
    let pb = std::path::PathBuf::from(path);
    let mut normalized = std::path::PathBuf::new();

    for comp in pb.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !normalized.pop() { normalized.push(comp); }
            }
            _ => normalized.push(comp),
        }
    }

    let result = normalized.to_string_lossy().to_string();
    let trimmed = result.trim_end_matches('/');
    if trimmed.is_empty() { "/".to_string() } else { trimmed.to_string() }
}
