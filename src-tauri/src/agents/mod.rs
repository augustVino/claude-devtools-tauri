//! Agent 适配层 —— 多 agent 会话支持的统一入口。
//!
//! # 架构定位（防腐层 / Anti-Corruption Layer）
//!
//! 各 agent 工具（Claude Code / Codex CLI / OpenCode / Pi / dsh）的本地会话
//! 格式互不兼容。本模块把「格式知识」收敛到各家的 adapter 内部，向下游
//! （分类、Chunk 构建、瀑布图、上下文追踪、前端渲染）只暴露统一的
//! [`ParsedMessage`] 中间表示 —— 下游管线不感知会话来自哪家 agent。
//!
//! # Adapter 输出契约
//!
//! adapter 输出的 `ParsedMessage` 必须满足以下字段契约。契约分三类：
//!
//! ## 1. 中立字段（每家 adapter 必须尽力填充）
//!
//! | 字段 | 语义 |
//! |------|------|
//! | `uuid` / `timestamp` / `role` / `model` / `usage` | 会话内稳定 id、ISO 8601 时间、角色、模型、token 用量 |
//! | `content` | **块协议**：`text` / `thinking` / `tool_use` / `tool_result` 四种块的 JSON 数组（Anthropic 块协议即本项目的通用语；各家原生块类型映射到此协议后，下游 tool 配对、瀑布、上下文追踪即可直接工作） |
//! | `tool_calls` / `tool_results` | 从 content 抽取的结构化工具调用/结果（`extract_tool_calls` 语义） |
//! | `cwd` / `git_branch` | 会话所属项目路径与分支 |
//!
//! ## 2. 泛化语义字段（名字沿自 Claude，语义中立；各家用自家证据填充）
//!
//! | 字段 | 中立语义 | 各家证据示例 |
//! |------|---------|-------------|
//! | `is_meta` | 非真人输入（注入上下文/环境回显），下游折叠展示 | Claude: `isMeta` 列；Codex: 内容前缀（`<environment_context` 等）；OpenCode: `synthetic` 列；dsh: `source.kind != "user"` |
//! | `is_compact_summary` | 上下文压缩产生的摘要消息 | Claude: `isCompactSummary`；Codex: `compacted` 行 |
//! | `is_sidechain` | 子代理（subagent）消息 | Claude: `isSidechain`；OpenCode: `parent_id`；dsh: `origin=subagent` |
//!
//! ## 3. Claude 特有字段（其他 adapter 一律输出 None / 空值，下游遇空短路）
//!
//! `request_id`（含**去重**：Claude 流式写入的重复行必须在 adapter 内按
//! requestId 去重后才输出 —— 这是 Claude 语义，不得泄漏为下游职责）、
//! `parent_uuid`、`source_tool_use_id`、`source_tool_assistant_uuid`、
//! `tool_use_result`、`user_type`、`agent_id`。
//!
//! # 跨 agent 项目归并（重要：为什么不用 id 同构）
//!
//! Claude 的项目目录名编码规则与其 CLI 版本相关（本机实证 `/Users/x/.claude`
//! → `-Users-x--claude`，`.` 亦编码为 `-`；而本项目 `encode_path` 仅编码
//! 路径分隔符），**任何 cwd 含 `.`/`_`/空格等字符时自编码 id 与 Claude
//! 目录名必然分叉**。因此归并的唯一依据是 **cwd 权威路径**：
//!
//! - Claude 侧：`Project.path` 来自会话文件内的 cwd 字段（权威，非目录名反解）；
//! - 其他 agent 侧：[`AgentSessionEntry::project_path`]（同样来自文件内容）；
//! - 匹配 = 路径字符串相等；命中则并入 Claude 项目（沿用其 id），未命中则
//!   以自编码 id（`encode_path(cwd)`）独立成项 —— 自编码 id **仅保证本模块
//!   内部一致**（list/locate 复用同一函数），不承诺与 Claude 目录名一致。
//!
//! # 覆盖范围声明（截至 P3）
//!
//! 已接入 Pi / Codex / OpenCode 的入口：项目列表、会话列表、会话详情/
//! 指标/分组/瀑布、全局与项目内搜索、按 id/部分 id 查找、仓库分组。
//! 文件监听：Pi（本地+SSH 两层轮询）、Codex（本地 notify）；OpenCode 为
//! SQLite 型，无实时刷新（列表靠 TTL 兑底，见 opencode.rs 模块文档）。
//! 错误检测通知（error_detector）为 claude JSONL 语义，extra agent 会话
//! 不参与（入口按 `event.agent` 过滤）。
//! 其他边界：删除仅本地模式且 OpenCode 为只读 no-op；SSH 模式 OpenCode
//! 不可用（rusqlite 需本地访问，降级日志可查）。
//!
//! # 新增 agent 的检查单
//!
//! 1. 在 [`crate::types::domain::AgentKind`] 加变体（注意 `AgentKind::ALL`
//!    顺序即前端展示顺序）；
//! 2. 新建 `agents/<name>.rs` 实现 [`AgentAdapter`]，**在 parse 阶段消化
//!    自家噪声**（注入内容 → `is_meta`，压缩 → `is_compact_summary`），
//!    不得依赖下游 classifier 认识你的原生格式；
//! 3. 实现「多 agent 聚合协议」的**全部四个方法**（`data_root_under` /
//!    `scan_sessions` / `locate_session` / `light_session`），缺一个就会
//!    「项目有会话数但列表为空」；
//! 4. 在 [`create_adapters`] 注册；
//! 5. 不认识的行不要静默吞掉 —— 至少 `log::warn!`（schema 漂移金丝雀）。

mod claude;
mod codex;
mod opencode;
mod pi;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::infrastructure::fs_provider::FsProvider;
use crate::types::domain::{AgentKind, Project, Session};
use crate::types::messages::ParsedMessage;
use crate::utils::{encode_path, extract_base_dir, extract_project_name};

pub use claude::ClaudeAdapter;
pub use codex::CodexAdapter;
pub use opencode::OpencodeAdapter;
pub use pi::PiAdapter;

/// 单个 agent 工具的会话数据适配器。
///
/// 实现方负责：把该 agent 的本地会话文件解析为符合模块契约的
/// [`ParsedMessage`] 列表。所有文件读取必须走 `FsProvider`
/// （SSH 模式下本地 `std::fs` 读不到远程文件）。
pub trait AgentAdapter: Send + Sync {
    /// 本 adapter 服务的 agent 类型。
    fn kind(&self) -> AgentKind;

    /// 本家会话数据根目录（如 `~/.claude/projects`）。
    fn data_roots(&self) -> Vec<PathBuf>;

    /// 解析单个会话文件 → 统一中间表示。
    ///
    /// 契约：
    /// - 输出必须已按模块文档完成 Claude 特有语义的消化（如 requestId 去重）；
    /// - 文件不存在 / 读取失败 → 返回空 vec（与既有
    ///   `parse_session_file_with_provider` 的容错语义一致，不向上抛错）。
    fn parse_messages(&self, path: &Path, fs: &dyn FsProvider) -> Vec<ParsedMessage>;

    /// 路径结构特征判定：该路径是否属于本家数据（用于 dispatch 与 agent 标记）。
    ///
    /// 用路径**结构特征**（如包含 `.pi/agent/sessions` 组件序列）而非绝对前缀：
    /// SSH 远端路径同样命中。默认 false；未命中任何 adapter 的路径回退
    /// registry 首家（claude，向后兼容）。
    fn owns_path(&self, path: &Path) -> bool {
        let _ = path;
        false
    }

    // ── 多 agent 聚合协议（四个方法，接入聚合必须全部实现）──────────
    //
    // 服务「项目/会话列表聚合层」（project_service / session_searcher）：
    // 枚举本家会话、构造 light 元数据、按 session_id 定位文件。Claude 不
    // 接入（走 ProjectScanner 既有管线，聚合层对 claude 不重复扫描）。

    /// 本家数据根相对 home 的位置（如 `~/.pi/agent/sessions`）。
    /// 调用方传入当前上下文（本地/SSH）的 home 目录。
    fn data_root_under(&self, home: &Path) -> PathBuf {
        let _ = home;
        PathBuf::new()
    }

    /// 枚举本家全部会话文件（必须廉价：stat + 首行读取，绝不全量解析）。
    /// `root` 为 [`Self::data_root_under`] 相对当前上下文 home 的实际路径。
    ///
    /// 契约：每个 entry 的 `project_path` 必须来自**该文件自身内容**的
    /// cwd（权威），不得用目录名反推 —— 目录编码可能有损/与他家不同构。
    fn scan_sessions(&self, root: &Path, fs: &dyn FsProvider) -> Vec<AgentSessionEntry> {
        let _ = (root, fs);
        Vec::new()
    }

    /// 按 session_id 定位会话文件（详情/删除等入口）。
    ///
    /// 实现必须容忍无法从统一项目 id 直推目录的布局（如 pi：枚举匹配
    /// 文件名）。`id_matches(cwd)` 由调用方提供：闭包内封装「该 cwd 是否
    /// 属于目标项目」的判定（自编码 id 相等 或 与 Claude 项目权威 cwd
    /// 相等）—— 实现对每个候选文件读取其 cwd 后必须过此校验，防止
    /// 跨项目误命中。
    fn locate_session(
        &self,
        root: &Path,
        session_id: &str,
        fs: &dyn FsProvider,
        id_matches: &dyn Fn(&str) -> bool,
    ) -> Option<PathBuf> {
        let _ = (root, session_id, fs, id_matches);
        None
    }

    /// 扫描产物 → light 元数据 Session（对齐 claude
    /// `build_session_for_listing` 的字段集与计数口径）。
    fn light_session(&self, entry: &AgentSessionEntry, fs: &dyn FsProvider) -> Option<Session> {
        let _ = (entry, fs);
        None
    }

    /// 实时监听根（相对当前上下文 home）。默认 = [`Self::data_root_under`]；
    /// 需要多子根的布局（如 codex 的 sessions + archived_sessions）覆写。
    /// local notify 递归监听这些根；ssh 轮询仅支持两层布局的根（深层树成本
    /// 爆炸，见 [`crate::agents::watch_roots`]）。
    fn watch_roots_under(&self, home: &Path) -> Vec<PathBuf> {
        vec![self.data_root_under(home)]
    }

    /// watcher 事件路径 → `(session_id, cwd)`。同步读文件头；
    /// None = 非会话文件（边车/噪声）或头损坏（半写）。claude 不实现
    /// （走既有 parse_path_parts 管线）。
    fn resolve_watch_event(
        &self,
        _path: &Path,
        _fs: &dyn FsProvider,
    ) -> Option<(String, String)> {
        None
    }
}

/// 扫描产物：单个会话文件的轻量元数据。
///
/// `project_id` 为**自编码**（`encode_path(cwd)`），仅保证本模块内部一致；
/// 跨 agent 归并一律以 `project_path`（文件内容中的 cwd 权威原文）匹配，
/// 不得依赖 id 与 Claude 目录名同构（见模块文档「跨 agent 项目归并」）。
#[derive(Debug, Clone)]
pub struct AgentSessionEntry {
    pub agent: AgentKind,
    pub project_id: String,
    pub project_path: String,
    pub session_id: String,
    pub file_path: PathBuf,
    pub mtime_ms: u64,
    pub birthtime_ms: u64,
    /// 会话首行声明的创建时间（epoch ms；0 = 未知，调用方按 mtime 兜底）。
    /// 由 scan 阶段读取并随 entry 携带，light 构造不再重读文件头。
    pub created_ms: u64,
}

/// 全量 adapter 注册表（进程级单例）。
///
/// 顺序即 `AgentKind::ALL` 的展示序。每新增一家 agent 在此追加，
/// dispatch 逻辑（[`adapter_for_path`]）随之自动覆盖。
/// 注：Claude 的项目/列表扫描仍走 ProjectScanner 既有管线（见 trait 中
/// 「多 agent 聚合协议」说明），聚合层对 claude 不重复扫描。
pub fn create_adapters() -> Vec<Arc<dyn AgentAdapter>> {
    vec![
        Arc::new(ClaudeAdapter::new()),
        Arc::new(CodexAdapter::new()),
        Arc::new(OpencodeAdapter::new()),
        Arc::new(PiAdapter::new()),
    ]
}

fn registry() -> &'static [Arc<dyn AgentAdapter>] {
    static REGISTRY: OnceLock<Vec<Arc<dyn AgentAdapter>>> = OnceLock::new();
    REGISTRY.get_or_init(create_adapters)
}

/// 路径 → 适配器。按 `owns_path` 结构特征分派；未命中时回退 registry 首家
/// （claude，向后兼容：旧数据路径无 pi 特征）。
pub fn adapter_for_path(path: &Path) -> &'static dyn AgentAdapter {
    registry()
        .iter()
        .find(|a| a.owns_path(path))
        .map(|a| a.as_ref())
        .unwrap_or_else(|| registry()[0].as_ref())
}

/// 路径 → agent 类型（[`adapter_for_path`] 的便捷封装）。
pub fn agent_for_path(path: &Path) -> AgentKind {
    adapter_for_path(path).kind()
}

/// 路径组件序列匹配（`owns_path` 的共享实现）。
///
/// 按组件精确匹配（如 `[".pi", "agent", "sessions"]`），本地与 SSH 远端
/// 路径通用。非 UTF-8 组件会被跳过（`filter_map` 语义）。
pub(crate) fn path_has_components(path: &Path, seq: &[&str]) -> bool {
    let comps: Vec<&str> = path
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    comps.windows(seq.len()).any(|w| w == seq)
}

/// 实时监听根：当前上下文存在的 extra agent 根（claude 除外，主 watcher
/// 已监听 projects_dir）。
///
/// SSH 轮询只支持两层布局（根/项目目录/文件）—— codex 的日期树
/// （sessions/YYYY/MM/DD，百级目录 × 3s 轮询 = SFTP readdir 爆炸）排除，
/// SSH 下 codex 会话无实时刷新（拉模式可用）；local notify 递归监听
/// 全部根不受此限。
pub fn watch_roots(home: &Path, fs: &dyn FsProvider) -> Vec<PathBuf> {
    if home.as_os_str().is_empty() {
        return Vec::new();
    }
    let is_ssh = fs.provider_type() == "ssh";
    let mut roots = Vec::new();
    for adapter in registry() {
        if adapter.kind() == AgentKind::ClaudeCode {
            continue;
        }
        if is_ssh && adapter.kind() == AgentKind::Codex {
            log::info!("agents: codex realtime watch skipped in SSH mode (date-tree polling too costly)");
            continue;
        }
        for root in adapter.watch_roots_under(home) {
            if root.as_os_str().is_empty() {
                continue;
            }
            if fs.exists(&root).unwrap_or(false) {
                roots.push(root);
            }
        }
    }
    roots
}

// ── 多 agent 聚合层 ──────────────────────────────────────────────────
//
// 服务 project_service / session_searcher / session_service：扫描接入聚合
// 协议的 agent（Pi 起；claude 走 ProjectScanner 既有管线不重复扫描），
// 按 cwd 权威路径与 claude 结果归并（见模块文档）。

/// 聚合层缓存：SSH 模式下每次 list/scan 全量重扫（逐会话 SFTP 头读）会让
/// 侧栏刷新风暴变成秒级往返串，这里用「TTL 结果缓存 + 事件失效」挡住重复
/// 调用：实时性由 watcher 事件回调 invalidate 保证（本地 notify / SSH 轮询），
/// TTL 只是防漏网的兑底。
const AGG_CACHE_TTL: Duration = Duration::from_secs(120);

#[derive(Default)]
struct AggCache {
    /// root 路径 → (生成时刻, 扫描结果)。TTL 过期后重扫（scan 本身仍走
    /// adapter，代价一次付清而非每调用）。
    scan: std::collections::HashMap<String, (Instant, Vec<AgentSessionEntry>)>,
    /// 数据根存在性（含**负缓存**）：远端未装某 agent 时，每次聚合调用
    /// 的 exists 探测也是一次 SFTP 往返 —— 高延迟链路上搜索/翻页风暴会把它
    /// 放大成秒级卡顿（2026-08 SSH 实测）。负结果同样缓存，稳态零往返。
    root_exists: std::collections::HashMap<String, (Instant, bool)>,
    /// file_path → (mtime_ms, light Session)：mtime 未变不重读 200 行预览。
    light: std::collections::HashMap<String, (u64, Session)>,
    /// (projects_dir, project_id) → (生成时刻, cwd)：claude 项目权威 cwd
    /// 的首文件头读同样昂贵。
    claude_cwd: std::collections::HashMap<String, (Instant, Option<String>)>,
}

impl AggCache {
    fn evict_if_bloated(&mut self) {
        // 粗粒度上限：超额全清（重建成本低，频率低）
        if self.light.len() > 8192 {
            self.light.clear();
        }
    }
}

fn agg_cache() -> &'static Mutex<AggCache> {
    static CACHE: OnceLock<Mutex<AggCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(AggCache::default()))
}

/// 有界并行 map（SSH 逐文件串行头读是聚合扫描的瓶颈：N 会话 = N 次
/// 串行往返，2026-08 实测 117 会话拖 24s）。
///
/// 语义：**worker pool** —— `workers` 个线程从共享队列取项，项级调度
///（任意项数都能吃到全部并发度；曾经的 chunk 分批实现批内串行：
/// 3 项挤单线程 468ms，被并发性回归测试抓到）。队列/结果锁只在取项/
/// 放回瞬间持有（纳秒级），`f` 在锁外执行 —— 调用方务必维持同样的锁
/// 纪律（见 `cached_light_session` 文档）。结果保序。
///
/// 聚合层调用点均在 spawn_blocking 线程，`SshFsProvider::blocking_sftp`
/// 在非 runtime 线程走 `handle.block_on`，多线程并发 block_on = SFTP
/// 请求在 SSH 通道上多路复用；本地 LocalFsProvider 同样安全。
pub(crate) fn par_map<T, R>(items: Vec<T>, workers: usize, f: impl Fn(T) -> R + Sync + Send) -> Vec<R>
where
    T: Send,
    R: Send,
{
    let n = items.len();
    if n <= 1 || workers <= 1 {
        return items.into_iter().map(f).collect();
    }
    let workers = workers.min(n);
    let queue: std::sync::Mutex<std::collections::VecDeque<(usize, T)>> =
        std::sync::Mutex::new(items.into_iter().enumerate().collect());
    let results: std::sync::Mutex<Vec<Option<R>>> =
        std::sync::Mutex::new((0..n).map(|_| None).collect());
    let f = &f;
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| loop {
                // 队列锁：仅 pop 瞬间持有
                let next = { queue.lock().unwrap().pop_front() };
                let Some((idx, item)) = next else { break };
                let r = f(item); // 用户函数在锁外
                // 结果锁：仅写入瞬间持有
                results.lock().unwrap()[idx] = Some(r);
            });
        }
    });
    results
        .into_inner()
        .expect("par_map results mutex poisoned")
        .into_iter()
        .map(|x| x.expect("par_map worker left slot unfilled"))
        .collect()
}

/// 扫描单飞锁：TTL 失效后的首个调用者执行扫描，并发调用者等锁后直接
/// 命中其写入的缓存 —— 消灭启动时 N 个并发调用（getProjects /
/// getRepositoryGroups / getSessions 同时打进来）各自全量扫描的
/// stampede（2026-08 SSH 实测：同一秒 3 条重复扫描日志）。
static SCAN_SINGLE_FLIGHT: Mutex<()> = Mutex::new(());

/// watcher 事件失效入口：extra agent 文件变更时调用（path 命中的根的
/// scan 缓存 + 该文件的 light 缓存）。简单起见不做前缀细分 —— 重建一次
/// 扫描的成本远低于错数据的成本。
pub fn invalidate_aggregate_cache(path: Option<&Path>) {
    let mut c = agg_cache().lock().unwrap();
    match path {
        Some(p) => {
            let ps = p.to_string_lossy().to_string();
            c.scan.retain(|root, _| !ps.starts_with(root.as_str()));
            // root_exists 一并失效：新建的 agent 数据目录下的首个文件事件
            // 应立即激活扫描（负缓存不得拖到 TTL）
            c.root_exists.retain(|root, _| !ps.starts_with(root.as_str()));
            c.light.remove(&ps);
        }
        None => {
            c.scan.clear();
            c.root_exists.clear();
            c.light.clear();
        }
    }
}

/// 取 adapter 在当前上下文的实际数据根；根缺失时 None（debug 日志）。
fn extra_scan_root(adapter: &dyn AgentAdapter, home: &Path, fs: &dyn FsProvider) -> Option<PathBuf> {
    let root = adapter.data_root_under(home);
    if root.as_os_str().is_empty() {
        return None;
    }
    if !fs.exists(&root).unwrap_or(false) {
        // 正常态：该 agent 未安装。debug 级别避免刷屏；排查「为什么没有
        // XX 的会话」时看这里即可区分「未装」与「装了但没数据」。
        log::debug!("agents: data root missing for {}: {}", adapter.kind(), root.display());
        return None;
    }
    Some(root)
}

/// 根存在性探测（带 TTL 缓存，含负缓存）。稳态下聚合调用**零 SFTP 往返**。
fn root_exists_cached(fs: &dyn FsProvider, root: &Path) -> bool {
    let key = root.to_string_lossy().to_string();
    {
        let cache = agg_cache().lock().unwrap();
        if let Some((at, exists)) = cache.root_exists.get(&key) {
            if at.elapsed() < AGG_CACHE_TTL {
                return *exists;
            }
        }
    }
    let exists = fs.exists(root).unwrap_or(false);
    agg_cache()
        .lock()
        .unwrap()
        .root_exists
        .insert(key, (Instant::now(), exists));
    exists
}

/// 一次扫描全部接入聚合协议的 agent（跳过 claude —— 其 scan_sessions
/// 为默认空实现，天然无开销）。
///
/// 三层防护挡住重复 IO：scan 结果 TTL 缓存 / 根存在性 TTL 缓存（含负
/// 缓存）/ 全局单飞锁（并发调用只有一个真正扫描，其余等锁后命中缓存）。
/// 稳态（TTL 内、无变更事件）零 SFTP 往返。fs IO 在数据锁外执行，
/// 单飞锁期间并发调用方阻塞等待（调用点均在 spawn_blocking，可接受）。
fn scan_extra_entries(fs: &dyn FsProvider, home: &Path) -> Vec<AgentSessionEntry> {
    if home.as_os_str().is_empty() {
        return Vec::new();
    }
    let _single_flight = SCAN_SINGLE_FLIGHT.lock().unwrap();
    let mut out: Vec<AgentSessionEntry> = Vec::new();
    for adapter in registry() {
        let root = adapter.data_root_under(home);
        if root.as_os_str().is_empty() {
            continue;
        }
        let root_key = root.to_string_lossy().to_string();
        // 1) scan 结果缓存命中 → 零 IO
        {
            let cache = agg_cache().lock().unwrap();
            if let Some((at, entries)) = cache.scan.get(&root_key) {
                if at.elapsed() < AGG_CACHE_TTL {
                    out.extend(entries.iter().cloned());
                    continue;
                }
            }
        }
        // 2) 根存在性（负缓存）：未装该 agent 的机器不付任何往返
        if !root_exists_cached(fs, &root) {
            continue;
        }
        // 3) 真实扫描（单飞锁持有中：并发调用方等锁而非重复扫）
        let started = Instant::now();
        let found = adapter.scan_sessions(&root, fs);
        // info 级：SSH 慢链路上这是列表首屏的主要成本项，需要可直接观测
        log::info!(
            "agents: scanned {} sessions for {} in {:?}",
            found.len(),
            adapter.kind(),
            started.elapsed()
        );
        // 根存在但零会话是正常态（如远端 ~/.codex 只有 config）——info 级，
        // 不刷 warn（2026-08 SSH 实测：空根每 TTL 周期重复告警困扰排查）
        if found.is_empty() {
            log::info!(
                "agents: {} data root exists but no sessions under {} (will retry after TTL)",
                adapter.kind(),
                root.display()
            );
        }
        agg_cache()
            .lock()
            .unwrap()
            .scan
            .insert(root_key, (Instant::now(), found.clone()));
        out.extend(found);
    }
    out
}

/// Claude 项目的权威 cwd：读项目目录下首个 jsonl 头部的 cwd 字段（带
/// TTL 缓存 —— 首文件头读在 SSH 上同样是一次往返）。
///
/// 这是「统一项目 id（Claude 目录名）→ cwd」的唯一可靠途径（目录名反解
/// 有损）。读不到（目录无 jsonl / 全部损坏）→ None，调用方降级为
/// 自编码 id 匹配。
fn claude_project_cwd(fs: &dyn FsProvider, projects_dir: &Path, project_id: &str) -> Option<String> {
    let cache_key = format!(
        "{}/{}",
        projects_dir.to_string_lossy(),
        extract_base_dir(project_id)
    );
    {
        let mut cache = agg_cache().lock().unwrap();
        if let Some((at, cwd)) = cache.claude_cwd.get(&cache_key) {
            if at.elapsed() < AGG_CACHE_TTL {
                return cwd.clone();
            }
        }
    }
    let dir = projects_dir.join(extract_base_dir(project_id));
    let resolved = (|| {
        if !fs.exists(&dir).unwrap_or(false) {
            return None;
        }
        let entries = fs.read_dir(&dir).ok()?;
        let first_jsonl = entries
            .iter()
            .find(|e| e.is_file && e.name.ends_with(".jsonl"))?;
        let head = fs.read_file_head(&dir.join(&first_jsonl.name), 5).ok()?;
        for line in head.lines() {
            let Ok(row) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if let Some(cwd) = row.get("cwd").and_then(|v| v.as_str()) {
                if !cwd.is_empty() {
                    return Some(cwd.to_string());
                }
            }
        }
        None
    })();
    let mut cache = agg_cache().lock().unwrap();
    cache.claude_cwd.insert(cache_key, (Instant::now(), resolved.clone()));
    resolved
}

/// entry 是否属于目标统一项目 id。
///
/// 双通道匹配：cwd 与 Claude 项目权威 cwd 相等（归并 case，覆盖自编码
/// 分叉的字符），或 自编码 id 相等（pi-only case）。
fn entry_belongs_to(
    e: &AgentSessionEntry,
    project_id: &str,
    claude_cwd: Option<&str>,
) -> bool {
    claude_cwd == Some(e.project_path.as_str()) || e.project_id == project_id
}

/// light 元数据构造（带 mtime 缓存）：预览读 200 行在 SSH 上是一次
/// SFTP 往返，mtime 未变直接复用。
///
/// 锁纪律（被违反过一次，务必维持）：Mutex **只在读/写缓存瞬间持有**，
/// `adapter.light_session` 的 fs IO 必须在锁外 —— 一旦 IO 入锁，外层
/// par_map 的 8 并发会被串行化成单流（2026-08 SSH 实测：117 会话
/// 24.3s，每会话 ~210ms = 单次 RTT，与串行无异；修复后应 ~N/8×RTT）。
fn cached_light_session(entry: &AgentSessionEntry, fs: &dyn FsProvider) -> Option<Session> {
    let path_key = entry.file_path.to_string_lossy().to_string();
    {
        let cache = agg_cache().lock().unwrap();
        if let Some((mtime, session)) = cache.light.get(&path_key) {
            if *mtime == entry.mtime_ms {
                return Some(session.clone());
            }
        }
    } // 锁释放 —— 下面的 SFTP IO 在锁外，可安全并发
    let adapter = registry()
        .iter()
        .find(|a| a.kind() == entry.agent)?;
    let session = adapter.light_session(entry, fs)?;
    let mut cache = agg_cache().lock().unwrap();
    cache.evict_if_bloated();
    cache.light.insert(path_key, (entry.mtime_ms, session.clone()));
    Some(session)
}

/// 在 claude 项目列表上归并额外 agent 的项目。
///
/// 归并依据：**cwd 路径相等**（见模块文档）——命中则并入 Claude 项目
/// （沿用其 id），否则以自编码 id 独立成项。返回后接最近活跃排序。
/// 供 `ProjectService::scan_projects` / `get_repository_groups` 调用。
pub fn merge_extra_projects(
    claude_projects: Vec<Project>,
    fs: &dyn FsProvider,
    home: &Path,
) -> Vec<Project> {
    let mut merged = claude_projects;
    if home.as_os_str().is_empty() {
        return merged;
    }
    let entries = scan_extra_entries(fs, home);
    if entries.is_empty() {
        return merged;
    }

    // 按 cwd 分组（不同目录可能落到同 cwd —— 以文件自身声明为准）
    let mut by_cwd: std::collections::HashMap<&str, Vec<&AgentSessionEntry>> =
        std::collections::HashMap::new();
    for e in &entries {
        by_cwd.entry(e.project_path.as_str()).or_default().push(e);
    }
    for (cwd, group) in by_cwd {
        let id = encode_path(cwd);
        let name = extract_project_name(&id, Some(cwd));
        if let Some(existing) = merged.iter_mut().find(|p| p.path == cwd) {
            // 归并：sessions 去重追加（claude 与 pi 的 session id 空间不同，
            // 直接 extend 不会碰撞）
            for e in &group {
                if !existing.sessions.contains(&e.session_id) {
                    existing.sessions.push(e.session_id.clone());
                }
            }
            existing.most_recent_session = Some(
                existing
                    .most_recent_session
                    .unwrap_or(0)
                    .max(group.iter().map(|e| e.mtime_ms).max().unwrap_or(0)),
            );
        } else {
            merged.push(Project {
                id,
                path: cwd.to_string(),
                name,
                sessions: group.iter().map(|e| e.session_id.clone()).collect(),
                created_at: group.iter().map(|e| e.birthtime_ms).filter(|t| *t > 0).min().unwrap_or(0),
                most_recent_session: Some(group.iter().map(|e| e.mtime_ms).max().unwrap_or(0)),
            });
        }
    }
    merged.sort_by(|a, b| {
        b.most_recent_session
            .unwrap_or(0)
            .cmp(&a.most_recent_session.unwrap_or(0))
    });
    merged
}

/// 追加额外 agent 的会话到 claude 会话列表（指定项目），按 updatedAt 降序。
/// 供 `ProjectService::list_sessions` 调用。
pub fn append_extra_sessions(
    mut sessions: Vec<Session>,
    fs: &dyn FsProvider,
    projects_dir: &Path,
    home: &Path,
    project_id: &str,
) -> Vec<Session> {
    if home.as_os_str().is_empty() {
        return sessions;
    }
    let claude_cwd = claude_project_cwd(fs, projects_dir, project_id);
    let matched: Vec<AgentSessionEntry> = scan_extra_entries(fs, home)
        .into_iter()
        .filter(|e| entry_belongs_to(e, project_id, claude_cwd.as_deref()))
        .collect();
    // light 构造（缓存 miss 时每会话一次 preview 头读）并行化：串行时
    // N 会话 = N 次 SFTP 往返叠加，31 会话实测把列表首开拖到 ~10s
    //（2026-08 SSH）。缓存命中路径纯内存，并行无害
    let extra = crate::agents::par_map(matched, 8, |e| cached_light_session(&e, fs))
        .into_iter()
        .flatten();
    sessions.extend(extra);
    sessions.sort_by(|a, b| b.updated_at.unwrap_or(0).cmp(&a.updated_at.unwrap_or(0)));
    sessions
}

/// 在 claude 直推路径之外定位额外 agent 的会话文件。
/// 供 `SessionService::session_path` fallback 调用：claude 路径 miss 时
/// 依次尝试各家 locate（带项目归属校验）。
pub fn locate_extra_session(
    projects_dir: &Path,
    home: &Path,
    project_id: &str,
    session_id: &str,
    fs: &dyn FsProvider,
) -> Option<PathBuf> {
    if home.as_os_str().is_empty() {
        return None;
    }
    let claude_cwd = claude_project_cwd(fs, projects_dir, project_id);
    let id_matches = move |cwd: &str| {
        encode_path(cwd) == project_id || claude_cwd.as_deref() == Some(cwd)
    };
    for adapter in registry() {
        let Some(root) = extra_scan_root(adapter.as_ref(), home, fs) else {
            continue;
        };
        if let Some(p) = adapter.locate_session(&root, session_id, fs, &id_matches) {
            return Some(p);
        }
    }
    None
}

/// 追加额外 agent 在指定项目下的搜索文件元组 `(session_id, path, mtime)`。
/// 供 `SessionSearcher::search_sessions` 追加非 claude 会话（复用既有
/// 匹配/分页机制）。session_id 由 adapter 侧提取（pi 文件名带时间戳
/// 前缀，不能沿用 claude 的 stem 直取）。
pub fn extra_search_files(
    fs: &dyn FsProvider,
    projects_dir: &Path,
    home: &Path,
    project_id: &str,
) -> Vec<(String, PathBuf, u64)> {
    if home.as_os_str().is_empty() {
        return Vec::new();
    }
    let claude_cwd = claude_project_cwd(fs, projects_dir, project_id);
    scan_extra_entries(fs, home)
        .into_iter()
        .filter(|e| entry_belongs_to(e, project_id, claude_cwd.as_deref()))
        .map(|e| (e.session_id, e.file_path, e.mtime_ms))
        .collect()
}

/// 按 session_id 在额外 agent 中查找会话（精确/前缀两种模式）。
/// 供 `SessionSearcher::find_session_by_id` / `find_sessions_by_partial_id`
/// 追加非 claude 结果。返回 (resolved_project_id, light Session)。
/// resolved id：cwd 命中的 claude 项目 id（保证前端后续 list/open 顺畅），
/// 无 claude 项目时为自编码 id。
pub fn find_extra_sessions(
    fs: &dyn FsProvider,
    projects_dir: &Path,
    home: &Path,
    session_id: &str,
    partial: bool,
) -> Vec<(String, Session)> {
    if home.as_os_str().is_empty() {
        return Vec::new();
    }
    let entries = scan_extra_entries(fs, home);
    let matched: Vec<&AgentSessionEntry> = entries
        .iter()
        .filter(|e| {
            if partial {
                e.session_id.starts_with(session_id)
            } else {
                e.session_id == session_id
            }
        })
        .collect();
    if matched.is_empty() {
        return Vec::new();
    }
    // cwd → claude 项目 id 反查（find 低频，接受 N 次 head 读）
    let claude_dirs = fs.read_dir(projects_dir).ok().map(|dirs| {
        dirs.iter()
            .filter(|d| d.is_directory)
            .map(|d| d.name.clone())
            .collect::<Vec<_>>()
    });
    let mut out = Vec::new();
    for e in matched {
        let Some(light) = cached_light_session(e, fs) else {
            continue;
        };
        let mut resolved = encode_path(&e.project_path);
        if let Some(dirs) = &claude_dirs {
            for d in dirs {
                if claude_project_cwd(fs, projects_dir, d).as_deref() == Some(e.project_path.as_str())
                {
                    resolved = d.clone();
                    break;
                }
            }
        }
        out.push((resolved, light));
    }
    out
}

/// 路径 → adapter 解析 → 聚合为 [`crate::parsing::ParsedSession`]。
///
/// 是 session_service 各解析入口的统一替换点（对齐旧
/// `parse_session_file_with_provider` 的输出形态），按路径分派 adapter。
pub fn parse_session_for(path: &Path, fs: &dyn FsProvider) -> crate::parsing::ParsedSession {
    let messages = adapter_for_path(path).parse_messages(path, fs);
    crate::parsing::process_messages(&messages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::fs_provider::LocalFsProvider;
    use std::sync::Arc;

    #[test]
    fn registry_contains_all_registered_kinds_in_order() {
        let adapters = create_adapters();
        assert_eq!(adapters.len(), 4);
        assert_eq!(adapters[0].kind(), AgentKind::ClaudeCode);
        assert_eq!(adapters[1].kind(), AgentKind::Codex);
        assert_eq!(adapters[2].kind(), AgentKind::Opencode);
        assert_eq!(adapters[3].kind(), AgentKind::Pi);
    }

    #[test]
    fn adapter_for_path_dispatches_by_path_structure() {
        let claude_path = Path::new("/Users/x/.claude/projects/-proj/s1.jsonl");
        assert_eq!(adapter_for_path(claude_path).kind(), AgentKind::ClaudeCode);
        // SSH 远端视角的 pi 路径同样命中结构特征
        let pi_path = Path::new("/home/remote/.pi/agent/sessions/--some-dir--/2026-08-25T05-41-57-146Z_019fff01.jsonl");
        assert_eq!(adapter_for_path(pi_path).kind(), AgentKind::Pi);
        // 未知路径回退 claude（向后兼容）
        assert_eq!(agent_for_path(Path::new("/unknown/path.jsonl")), AgentKind::ClaudeCode);
    }

    // ── 归并回归测试（finding 1：编码分叉不拆项目）──────────────────

    /// 跨 agent 聚合测试脚手架：临时 home 下布置 claude + pi 数据。
    struct Fixture {
        home: PathBuf,
        projects_dir: PathBuf,
        _dir: tempfile::TempDir,
    }

    fn make_fixture(claude_dir: &str, claude_cwd: &str, pi_cwds: &[&str]) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().to_path_buf();
        let projects_dir = home.join(".claude").join("projects");
        let claude_proj = projects_dir.join(claude_dir);
        std::fs::create_dir_all(&claude_proj).unwrap();
        std::fs::write(
            claude_proj.join("11111111-1111-4111-8111-111111111111.jsonl"),
            format!(
                "{{\"type\":\"user\",\"uuid\":\"u1\",\"cwd\":\"{claude_cwd}\",\"message\":{{\"role\":\"user\",\"content\":\"hi\"}}}}\n"
            ),
        )
        .unwrap();
        let pi_root = home.join(".pi").join("agent").join("sessions");
        for (i, cwd) in pi_cwds.iter().enumerate() {
            let proj = pi_root.join(format!("--dir{i}--"));
            std::fs::create_dir_all(&proj).unwrap();
            std::fs::write(
                proj.join(format!("2026-08-25T00-00-00-000Z_0000000{i}-0000-4000-8000-00000000000{i}.jsonl")),
                format!(
                    "{{\"type\":\"session\",\"version\":3,\"id\":\"0000000{i}-0000-4000-8000-00000000000{i}\",\"timestamp\":\"2026-08-25T00:00:00.000Z\",\"cwd\":\"{cwd}\"}}\n{{\"type\":\"message\",\"id\":\"m1\",\"timestamp\":\"2026-08-25T00:00:01.000Z\",\"message\":{{\"role\":\"user\",\"content\":[{{\"type\":\"text\",\"text\":\"pi hello\"}}]}}}}\n"
                ),
            )
            .unwrap();
        }
        Fixture { home, projects_dir, _dir: dir }
    }

    fn claude_projects(fx: &Fixture) -> Vec<Project> {
        vec![Project {
            id: "CLAUDE_DIR".into(), // 占位，下面替换
            path: String::new(),
            name: String::new(),
            sessions: vec![],
            created_at: 0,
            most_recent_session: None,
        }]
        .into_iter()
        .map(|mut p| {
            // 用 scanner 语义构造一个带权威 path 的 claude 项目
            let dir_name = fx
                .projects_dir
                .read_dir()
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .to_string();
            p.id = dir_name;
            p
        })
        .collect()
    }

    /// cwd 含 `.`：claude 目录名与 encode_path 分叉，归并仍必须命中同一条目。
    #[test]
    fn merge_by_path_when_encoding_diverges() {
        // claude 目录名 `-Users-x-my-repo`（`.` 被编码），cwd 实为 /Users/x/my.repo
        let fx = make_fixture("-Users-x-my-repo", "/Users/x/my.repo", &["/Users/x/my.repo"]);
        let mut projects = claude_projects(&fx);
        // 权威 path（来自 claude session 的 cwd）
        projects[0].path = "/Users/x/my.repo".into();
        projects[0].sessions = vec!["11111111-1111-4111-8111-111111111111".into()];

        let fs = Arc::new(LocalFsProvider::new());
        let merged = merge_extra_projects(projects, fs.as_ref(), &fx.home);

        assert_eq!(merged.len(), 1, "编码分叉不得拆出重复项目");
        assert_eq!(merged[0].id, "-Users-x-my-repo", "归并沿用 claude 项目 id");
        assert_eq!(merged[0].sessions.len(), 2, "claude + pi 会话都在同一项目下");

        // list：claude id 寻址也能带出 pi 会话（cwd 双通道匹配）
        let sessions = append_extra_sessions(
            Vec::new(),
            fs.as_ref(),
            &fx.projects_dir,
            &fx.home,
            "-Users-x-my-repo",
        );
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].agent, AgentKind::Pi);

        // locate：claude id + pi session id 也能定位到文件
        let found = locate_extra_session(
            &fx.projects_dir,
            &fx.home,
            "-Users-x-my-repo",
            "00000000-0000-4000-8000-000000000000",
            fs.as_ref(),
        );
        assert!(found.is_some(), "claude id 必须能定位归并的 pi 会话文件");
    }

    /// pi-only 项目（无 claude 目录）：自编码 id 内部一致。
    #[test]
    fn pi_only_project_uses_own_encoding_consistently() {
        let fx = make_fixture("-Users-x-empty", "/Users/x/empty", &["/Users/x/only-pi"]);
        let fs = Arc::new(LocalFsProvider::new());
        let merged = merge_extra_projects(Vec::new(), fs.as_ref(), &fx.home);

        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].id, "-Users-x-only-pi");
        assert_eq!(merged[0].path, "/Users/x/only-pi");

        let sessions = append_extra_sessions(
            Vec::new(),
            fs.as_ref(),
            &fx.projects_dir,
            &fx.home,
            "-Users-x-only-pi",
        );
        assert_eq!(sessions.len(), 1);

        let found = locate_extra_session(
            &fx.projects_dir,
            &fx.home,
            "-Users-x-only-pi",
            "00000000-0000-4000-8000-000000000000",
            fs.as_ref(),
        );
        assert!(found.is_some());
    }

    /// 归并项目同时有 claude 与 pi 会话时，find_extra_sessions 解析到 claude id。
    #[test]
    fn find_extra_resolves_to_claude_project_id() {
        let fx = make_fixture("-Users-x-my-repo", "/Users/x/my.repo", &["/Users/x/my.repo"]);
        let fs = Arc::new(LocalFsProvider::new());
        let found = find_extra_sessions(
            fs.as_ref(),
            &fx.projects_dir,
            &fx.home,
            "00000000-0000-4000-8000-000000000000",
            false,
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, "-Users-x-my-repo");
        assert_eq!(found[0].1.agent, AgentKind::Pi);
    }

    /// 计数 FsProvider：验证聚合层缓存真的挡住了重复 IO（含 exists 往返 ——
    /// SSH 高延迟链路上它就是搜索/翻页风暴变慢的元凶）。
    #[derive(Debug)]
    struct CountingFs {
        inner: Arc<crate::infrastructure::fs_provider::LocalFsProvider>,
        head_reads: std::sync::atomic::AtomicUsize,
        exists_calls: std::sync::atomic::AtomicUsize,
    }

    impl FsProvider for CountingFs {
        fn provider_type(&self) -> &'static str { "local" }
        fn exists(&self, p: &Path) -> Result<bool, String> {
            self.exists_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.exists(p)
        }
        fn read_file(&self, p: &Path) -> Result<String, String> { self.inner.read_file(p) }
        fn read_file_head(&self, p: &Path, n: usize) -> Result<String, String> {
            self.head_reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.read_file_head(p, n)
        }
        fn read_file_range(&self, p: &Path, o: u64, l: Option<u64>) -> Result<Vec<u8>, String> {
            self.inner.read_file_range(p, o, l)
        }
        fn stat(&self, p: &Path) -> Result<crate::infrastructure::fs_provider::FsStatResult, String> {
            self.inner.stat(p)
        }
        fn read_dir(&self, p: &Path) -> Result<Vec<crate::infrastructure::fs_provider::FsDirent>, String> {
            self.inner.read_dir(p)
        }
    }

    /// TTL 缓存：第二次 scan+light 全部命中缓存，零头读；失效后重扫。
    #[test]
    fn aggregate_cache_blocks_repeated_io() {
        let fx = make_fixture("-Users-x-cache", "/Users/x/cache", &["/Users/x/cache"]);
        let fs = Arc::new(CountingFs {
            inner: Arc::new(crate::infrastructure::fs_provider::LocalFsProvider::new()),
            head_reads: std::sync::atomic::AtomicUsize::new(0),
            exists_calls: std::sync::atomic::AtomicUsize::new(0),
        });
        // 隔离全局缓存（其他测试的 fixture 路径不同，理论上不冲突，显式清理更稳）
        invalidate_aggregate_cache(None);

        // 第一次：scan（1 次头读）+ light preview（1 次）= 2
        let merged = merge_extra_projects(Vec::new(), fs.as_ref(), &fx.home);
        assert_eq!(merged.len(), 1);
        let first = fs.head_reads.load(std::sync::atomic::Ordering::SeqCst);
        assert!(first >= 1, "first call must read heads");

        // 第二次（首个 append）：scan 已缓存，但 claude_cwd 与 light 首次
        // 填充合法读 2 次（cwd 头 + preview 头）
        let sessions = append_extra_sessions(Vec::new(), fs.as_ref(), &fx.projects_dir, &fx.home, "-Users-x-cache");
        assert_eq!(sessions.len(), 1);
        let after_warm = fs.head_reads.load(std::sync::atomic::Ordering::SeqCst);
        let exists_warm = fs.exists_calls.load(std::sync::atomic::Ordering::SeqCst);

        // 第三次：TTL 内全命中（scan + cwd + light + 根存在性），零头读零 exists
        let sessions = append_extra_sessions(Vec::new(), fs.as_ref(), &fx.projects_dir, &fx.home, "-Users-x-cache");
        assert_eq!(sessions.len(), 1);
        assert_eq!(
            fs.head_reads.load(std::sync::atomic::Ordering::SeqCst),
            after_warm,
            "third call within TTL must be served from cache (zero head reads)"
        );
        assert_eq!(
            fs.exists_calls.load(std::sync::atomic::Ordering::SeqCst),
            exists_warm,
            "steady-state aggregation must issue zero exists round-trips"
        );

        // 失效（模拟 watcher 事件）→ 重扫，头读数增长
        let changed = fx
            .home
            .join(".pi")
            .join("agent")
            .join("sessions")
            .join("--dir0--")
            .join("2026-08-25T00-00-00-000Z_00000000-0000-4000-8000-000000000000.jsonl");
        invalidate_aggregate_cache(Some(&changed));
        let _ = append_extra_sessions(Vec::new(), fs.as_ref(), &fx.projects_dir, &fx.home, "-Users-x-cache");
        assert!(
            fs.head_reads.load(std::sync::atomic::Ordering::SeqCst) > first,
            "after invalidation the scan must re-read"
        );
        invalidate_aggregate_cache(None);
    }

    /// 慢速 FsProvider：read_file_head 阻塞指定毫秒 —— 验证 light 构造的
    /// 并发性（锁纪律防回归：一旦 fs IO 被锁覆盖，par_map 退化串行，
    /// 2026-08 SSH 实测 117 会话 24.3s）。
    #[derive(Debug)]
    struct SlowFs {
        delay_ms: u64,
    }

    impl FsProvider for SlowFs {
        fn provider_type(&self) -> &'static str { "local" }
        fn exists(&self, _: &Path) -> Result<bool, String> { Ok(true) }
        fn read_file(&self, _: &Path) -> Result<String, String> { Ok(String::new()) }
        fn read_file_head(&self, _: &Path, _: usize) -> Result<String, String> {
            std::thread::sleep(std::time::Duration::from_millis(self.delay_ms));
            // 最小合法 pi 会话：1 条用户消息（preview 计数 = 1）
            Ok(concat!(
                r#"{"type":"session","cwd":"/slow","timestamp":"2026-08-25T00:00:00.000Z"}"#,
                "\n",
                r#"{"type":"message","id":"m1","timestamp":"2026-08-25T00:00:01.000Z","message":{"role":"user","content":[{"type":"text","text":"hi"}]}}"#,
                "\n",
            ).to_string())
        }
        fn read_file_range(&self, _: &Path, _: u64, _: Option<u64>) -> Result<Vec<u8>, String> {
            Ok(Vec::new())
        }
        fn stat(&self, _: &Path) -> Result<crate::infrastructure::fs_provider::FsStatResult, String> {
            Ok(crate::infrastructure::fs_provider::FsStatResult {
                size: 1, mtime_ms: 0, birthtime_ms: 0, is_file: true, is_directory: false,
            })
        }
        fn read_dir(&self, _: &Path) -> Result<Vec<crate::infrastructure::fs_provider::FsDirent>, String> {
            Ok(Vec::new())
        }
    }

    /// 并行性防回归：3 个不同 entry（缓存冷）× 150ms preview 读 —— 串行
    /// ≥450ms；真并行（批 8）应在单批延迟量级。阈值留宽（350ms）防 CI 抖动。
    #[test]
    fn light_construction_runs_concurrently_not_serialized() {
        invalidate_aggregate_cache(None);
        let fs = Arc::new(SlowFs { delay_ms: 150 });
        let mk_entry = |i: usize| AgentSessionEntry {
            agent: AgentKind::Pi,
            project_id: format!("/slow-{i}"),
            project_path: format!("/slow-{i}"),
            session_id: format!("s{i}"),
            file_path: PathBuf::from(format!("/slow-{i}/s{i}.jsonl")),
            mtime_ms: 0,
            birthtime_ms: 0,
            created_ms: 0,
        };
        let entries: Vec<AgentSessionEntry> = (0..3).map(mk_entry).collect();

        let started = std::time::Instant::now();
        let out: Vec<_> = par_map(entries, 8, |e| cached_light_session(&e, fs.as_ref()))
            .into_iter()
            .flatten()
            .collect();
        let elapsed = started.elapsed();

        assert_eq!(out.len(), 3, "all three lights must resolve");
        assert!(
            elapsed < std::time::Duration::from_millis(350),
            "3×150ms previews took {:?} — fs IO appears serialized by a lock (expect ~parallel 150ms, serial would be 450ms)",
            elapsed
        );
        invalidate_aggregate_cache(None);
    }

    /// watch_roots：本地模式含 pi + codex 双子根；SSH 排除 codex；缺根跳过。
    #[test]
    fn watch_roots_respect_context_and_existence() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().to_path_buf();
        // 只有 pi 根存在
        std::fs::create_dir_all(home.join(".pi").join("agent").join("sessions")).unwrap();

        let local = Arc::new(crate::infrastructure::fs_provider::LocalFsProvider::new());
        let roots = watch_roots(&home, local.as_ref());
        assert_eq!(roots, vec![home.join(".pi").join("agent").join("sessions")],
            "codex 根不存在 → 跳过；只返回存在的 pi 根");

        // codex sessions 存在 → local 模式返回 pi + codex sessions + archived
        std::fs::create_dir_all(home.join(".codex").join("sessions")).unwrap();
        let roots = watch_roots(&home, local.as_ref());
        assert_eq!(roots.len(), 2);
        assert!(roots.contains(&home.join(".codex").join("sessions")));

        // 空 home → 空 roots（SSH 自定义根降级）
        assert!(watch_roots(&Path::new(""), local.as_ref()).is_empty());
    }

    /// 搜索追加：归并项目下 pi 文件可见。
    #[test]
    fn extra_search_files_cover_merged_project() {
        let fx = make_fixture("-Users-x-my-repo", "/Users/x/my.repo", &["/Users/x/my.repo"]);
        let fs = Arc::new(LocalFsProvider::new());
        let files = extra_search_files(fs.as_ref(), &fx.projects_dir, &fx.home, "-Users-x-my-repo");
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].0,
            "00000000-0000-4000-8000-000000000000",
            "session_id 必须是 stem 尾段（不带时间戳前缀）"
        );
    }
}
