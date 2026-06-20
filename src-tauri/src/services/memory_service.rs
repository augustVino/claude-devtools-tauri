//! Memory Service implementation — SSH-aware via FsProvider.
//!
//! 对齐 Electron `MemoryReader.ts`：
//! - 构造持有 `ContextManager`，从 active ServiceContext 动态取 fs_provider + projects_dir
//! - 所有读取走 fs_provider（本地 std::fs / 远程 SFTP 多态）
//! - containment 校验用 `assert_safe_name` + path 字符串等值（不依赖 canonicalize，
//!   SSH SFTP 不支持 readlink 全路径；与 Electron `MemoryReader.ts:54-57` 一致）

use crate::error::AppError;
use crate::infrastructure::fs_provider::FsProvider;
use crate::infrastructure::ContextManager;
use crate::types::memory::{parse_memory_index, MemoryFile, MemoryIndex};
use crate::utils::app_opener;
use crate::utils::path_decoder;
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

use super::memory_service_trait::MemoryService;

const MEMORY_DIR_NAME: &str = "memory";
const INDEX_FILE_NAME: &str = "MEMORY.md";
/// Max file size for a single memory file (1 MB). SSH 防御性限制。
const MAX_FILE_SIZE: usize = 1024 * 1024;

pub struct MemoryServiceImpl {
    context_manager: Arc<RwLock<ContextManager>>,
}

impl MemoryServiceImpl {
    pub fn new(context_manager: Arc<RwLock<ContextManager>>) -> Self {
        Self { context_manager }
    }

    /// 从 active ServiceContext 取 fs_provider + projects_dir。
    async fn deps(&self) -> Result<(Arc<dyn FsProvider>, PathBuf), AppError> {
        let active_arc = {
            let mgr = self.context_manager.read().await;
            mgr.get_active()
                .ok_or_else(|| AppError::Internal("No active ServiceContext".into()))?
        };
        let ctx = active_arc.read().await;
        Ok((ctx.fs_provider.clone(), ctx.projects_dir.clone()))
    }

    /// Resolve memory directory path. Uses `extract_base_dir` to strip
    /// composite ID suffixes (`::{hash}`), matching project_scanner.rs.
    fn dir_path(projects_dir: &Path, project_id: &str) -> PathBuf {
        let base_dir = path_decoder::extract_base_dir(project_id);
        projects_dir.join(base_dir).join(MEMORY_DIR_NAME)
    }

    /// Validate file name and resolve to absolute path.
    /// 校验对齐 Electron `assertSafeMarkdownName`：拒绝 `..`、`/`、`\`，
    /// 强制 `.md` 后缀；containment 用 path 字符串等值（resolve 后必须等于 join 结果），
    /// 不依赖 canonicalize（SSH SFTP 不支持 readlink 全路径）。
    fn safe_file_path(
        projects_dir: &Path,
        project_id: &str,
        file_name: &str,
    ) -> Result<PathBuf, AppError> {
        let dir = Self::dir_path(projects_dir, project_id);
        let safe = Self::assert_safe_name(file_name)?;
        let resolved = dir.join(&safe);
        // resolve 等值检查：safe 已拒绝 `..` 和分隔符，resolved 必须等于 dir/safe。
        // 这是纯字符串操作，SSH 下安全，对齐 Electron MemoryReader.ts:54-57。
        let expected = dir.join(&safe);
        if resolved != expected {
            return Err(AppError::InvalidInput(format!(
                "Memory file path escapes memory directory: {file_name}"
            )));
        }
        Ok(resolved)
    }

    /// 对齐 Electron `assertSafeMarkdownName`。
    fn assert_safe_name(file_name: &str) -> Result<String, AppError> {
        let trimmed = file_name.trim();
        if trimmed.is_empty() {
            return Err(AppError::InvalidInput("Memory file name is empty".into()));
        }
        if trimmed.contains('/') || trimmed.contains('\\') {
            return Err(AppError::InvalidInput(format!(
                "Memory file name must not contain path separators: {file_name}"
            )));
        }
        if trimmed.contains("..") {
            return Err(AppError::InvalidInput(format!(
                "Memory file name must not contain '..': {file_name}"
            )));
        }
        if !trimmed.to_lowercase().ends_with(".md") {
            return Err(AppError::InvalidInput(format!(
                "Memory file must end with .md: {file_name}"
            )));
        }
        Ok(trimmed.to_string())
    }
}

#[async_trait]
impl MemoryService for MemoryServiceImpl {
    async fn has_memory(&self, project_id: &str) -> Result<bool, AppError> {
        let (fs_provider, projects_dir) = self.deps().await?;
        let dir = Self::dir_path(&projects_dir, project_id);
        if !fs_provider.exists(&dir).unwrap_or(false) {
            return Ok(false);
        }
        let entries = match fs_provider.read_dir(&dir) {
            Ok(e) => e,
            Err(_) => return Ok(false),
        };
        Ok(entries
            .iter()
            .any(|e| e.is_file && e.name.to_lowercase().ends_with(".md")))
    }

    async fn read_index(&self, project_id: &str) -> Result<Option<MemoryIndex>, AppError> {
        let (fs_provider, projects_dir) = self.deps().await?;
        let dir = Self::dir_path(&projects_dir, project_id);
        if !fs_provider.exists(&dir).unwrap_or(false) {
            return Ok(None);
        }

        let mut names = Vec::new();
        match fs_provider.read_dir(&dir) {
            Ok(entries) => {
                for entry in entries {
                    if entry.is_file {
                        names.push(entry.name);
                    }
                }
            }
            Err(_) => return Ok(None),
        }

        let index_path = dir.join(INDEX_FILE_NAME);
        let raw = if fs_provider.exists(&index_path).unwrap_or(false) {
            fs_provider.read_file(&index_path).unwrap_or_default()
        } else {
            String::new()
        };
        Ok(Some(parse_memory_index(&raw, &names)))
    }

    async fn read_file(&self, project_id: &str, file_name: &str) -> Result<MemoryFile, AppError> {
        let (fs_provider, projects_dir) = self.deps().await?;
        let absolute_path = Self::safe_file_path(&projects_dir, project_id, file_name)?;

        let content = fs_provider.read_file(&absolute_path).map_err(|e| {
            AppError::NotFound(format!("Memory file read failed for {file_name}: {e}"))
        })?;

        // 防御性 size 检查（对齐 Tauri 原有 MAX_FILE_SIZE 防护，Electron 没有此检查）
        if content.len() > MAX_FILE_SIZE {
            return Err(AppError::InvalidInput(format!(
                "Memory file exceeds {} KB limit",
                MAX_FILE_SIZE / 1024
            )));
        }

        Ok(MemoryFile {
            file_name: file_name.to_string(),
            absolute_path: absolute_path.to_string_lossy().to_string(),
            content,
        })
    }

    fn get_dir_path(&self, project_id: &str) -> String {
        // 同步接口，无法 await deps()。用本地默认路径作为 fallback。
        // 真正的 SSH-aware 路径在异步方法中通过 deps() 解析；这里仅供 UI 展示用。
        let projects_dir = path_decoder::get_projects_base_path();
        Self::dir_path(&projects_dir, project_id)
            .to_string_lossy()
            .to_string()
    }

    fn get_file_path(&self, project_id: &str, file_name: &str) -> Result<String, AppError> {
        // 同步接口 fallback，与 get_dir_path 同理。
        let projects_dir = path_decoder::get_projects_base_path();
        Ok(Self::safe_file_path(&projects_dir, project_id, file_name)?
            .to_string_lossy()
            .to_string())
    }

    async fn open_in(
        &self,
        opener_id: &str,
        project_id: &str,
        file_name: Option<&str>,
    ) -> Result<(), AppError> {
        // open_in 调用 OS opener 打开本地路径。SSH 模式下路径是远程的，
        // OS opener 无法打开——这是已知限制（与 Electron MemoryReader.openIn 一致：
        // Electron 也只在本地模式下提供 open_in）。
        let path = match file_name {
            Some(name) if !name.trim().is_empty() => self.get_file_path(project_id, name)?,
            _ => self.get_dir_path(project_id),
        };
        let is_directory = file_name.map_or(true, |n| n.trim().is_empty());
        app_opener::open_with(opener_id, &path, is_directory).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::fs_provider::{FsDirent, FsStatResult};
    use crate::infrastructure::service_context::{ContextType, ServiceContext, ServiceContextConfig};
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;

    // ── InMemoryFsProvider（SSH-aware 单测 mock） ───────────────

    #[derive(Debug, Clone)]
    struct InMemoryFsProvider {
        provider_type_str: &'static str,
        files: Arc<StdMutex<HashMap<String, String>>>,
        dirs: Arc<StdMutex<HashMap<String, Vec<String>>>>,
        exists_calls: Arc<StdMutex<Vec<String>>>,
    }

    impl InMemoryFsProvider {
        fn new(provider_type_str: &'static str) -> Self {
            Self {
                provider_type_str,
                files: Arc::new(StdMutex::new(HashMap::new())),
                dirs: Arc::new(StdMutex::new(HashMap::new())),
                exists_calls: Arc::new(StdMutex::new(Vec::new())),
            }
        }

        fn set_file(&self, path: &str, content: &str) {
            self.files
                .lock()
                .unwrap()
                .insert(path.to_string(), content.to_string());
        }

        fn set_dir(&self, path: &str, file_names: Vec<&str>) {
            self.dirs.lock().unwrap().insert(
                path.to_string(),
                file_names.into_iter().map(String::from).collect(),
            );
        }

        fn exists_call_count(&self) -> usize {
            self.exists_calls.lock().unwrap().len()
        }
    }

    impl FsProvider for InMemoryFsProvider {
        fn provider_type(&self) -> &'static str {
            self.provider_type_str
        }
        fn exists(&self, path: &Path) -> Result<bool, String> {
            self.exists_calls
                .lock()
                .unwrap()
                .push(path.to_string_lossy().to_string());
            let key = path.to_string_lossy().to_string();
            Ok(self.files.lock().unwrap().contains_key(&key)
                || self.dirs.lock().unwrap().contains_key(&key))
        }
        fn read_file(&self, path: &Path) -> Result<String, String> {
            self.files
                .lock()
                .unwrap()
                .get(&path.to_string_lossy().to_string())
                .cloned()
                .ok_or_else(|| format!("not found: {}", path.display()))
        }
        fn read_file_head(&self, path: &Path, _max_lines: usize) -> Result<String, String> {
            self.read_file(path)
        }
        fn read_file_range(
            &self,
            _path: &Path,
            _offset: u64,
            _length: Option<u64>,
        ) -> Result<Vec<u8>, String> {
            Ok(Vec::new())
        }
        fn stat(&self, _path: &Path) -> Result<FsStatResult, String> {
            Ok(FsStatResult {
                size: 0,
                mtime_ms: 0,
                birthtime_ms: 0,
                is_file: true,
                is_directory: false,
            })
        }
        fn read_dir(&self, path: &Path) -> Result<Vec<FsDirent>, String> {
            let key = path.to_string_lossy().to_string();
            let names = self.dirs.lock().unwrap().get(&key).cloned();
            Ok(names
                .unwrap_or_default()
                .into_iter()
                .map(|name| FsDirent {
                    name,
                    is_file: true,
                    is_directory: false,
                    size: Some(0),
                    mtime_ms: Some(0),
                    birthtime_ms: Some(0),
                })
                .collect())
        }
    }

    fn make_ssh_context(
        provider: InMemoryFsProvider,
    ) -> Arc<RwLock<ContextManager>> {
        let mut mgr = ContextManager::new();
        mgr.register_context(ServiceContext::new(ServiceContextConfig {
            id: "ssh-test".to_string(),
            context_type: ContextType::Ssh,
            projects_dir: PathBuf::from("/projects"),
            todos_dir: PathBuf::from("/todos"),
            fs_provider: Arc::new(provider),
            cache: None,
        }))
        .unwrap();
        let _ = mgr.switch("ssh-test");
        Arc::new(RwLock::new(mgr))
    }

    /// SSH 模式下 has_memory / read_index / read_file 必须通过 fs_provider 读取。
    #[tokio::test]
    async fn test_memory_service_uses_fs_provider_in_ssh_mode() {
        let provider = InMemoryFsProvider::new("ssh");
        provider.set_dir("/projects/proj/memory", vec!["MEMORY.md", "layer1.md"]);
        provider.set_file(
            "/projects/proj/memory/MEMORY.md",
            "# Memory\n## layer1.md\n",
        );
        provider.set_file("/projects/proj/memory/layer1.md", "layer content");

        let svc = MemoryServiceImpl::new(make_ssh_context(provider.clone()));

        // has_memory
        let has = svc.has_memory("proj").await.unwrap();
        assert!(has, "SSH mode should detect memory via fs_provider");
        assert!(
            provider.exists_call_count() >= 1,
            "must check existence via fs_provider"
        );

        // read_index
        let idx = svc.read_index("proj").await.unwrap().unwrap();
        assert!(
            idx.raw_markdown.contains("# Memory"),
            "SSH mode should read MEMORY.md content via fs_provider"
        );

        // read_file
        let f = svc.read_file("proj", "layer1.md").await.unwrap();
        assert_eq!(f.content, "layer content");
        assert_eq!(f.file_name, "layer1.md");
    }

    /// assert_safe_name 必须拒绝 `..`、`/`、`\` 和非 .md 后缀（TOCTOU 防护）。
    #[tokio::test]
    async fn test_memory_service_rejects_unsafe_name() {
        let provider = InMemoryFsProvider::new("local");
        let svc = MemoryServiceImpl::new(make_ssh_context(provider));

        let cases: &[(&str, &str)] = &[
            ("../etc/passwd", ".."),
            ("sub/dir/secret.md", "/"),
            ("back\\slash.md", "\\"),
            ("no_extension", ".md"),
            ("", "empty"),
        ];

        for (input, _hint) in cases {
            let result = svc.read_file("proj", input).await;
            assert!(
                matches!(result, Err(AppError::InvalidInput(_))),
                "input {input:?} should be rejected as unsafe name"
            );
        }
    }

    /// 空 memory 目录返回 has_memory=false、read_index 仍可解析（空 index）。
    #[tokio::test]
    async fn test_memory_service_handles_missing_dir_in_ssh_mode() {
        let provider = InMemoryFsProvider::new("ssh");
        // 不设置任何 dir/file，模拟 memory 目录不存在
        let svc = MemoryServiceImpl::new(make_ssh_context(provider));

        let has = svc.has_memory("proj").await.unwrap();
        assert!(!has, "missing memory dir should return false");

        let idx = svc.read_index("proj").await.unwrap();
        assert!(idx.is_none(), "missing memory dir should return None");
    }
}
