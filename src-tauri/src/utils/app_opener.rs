//! macOS 外部应用检测与打开工具。
//!
//! 检测 10 个 macOS 应用（Finder, Cursor, VS Code, Zed, Xcode,
//! Ghostty, iTerm, Terminal, Android Studio, Antigravity）的安装状态，
//! 并提供用指定应用打开文件/目录的功能。
//!
//! 非 macOS 平台：detect_installations 返回空 Vec，open_with 返回错误。
//!
//! ## 安全说明
//!
//! 所有 `Command::new()` 调用不使用 `sh -c`，参数通过 `.arg()` 逐个传入，
//! 因此不受 shell 注入影响。`opener_id` 通过 `OpenTargetId::from_str` 白名单校验，
//! 未知值直接返回 `Err`。

use crate::error::AppError;
use crate::types::memory::OpenTarget;
use std::str::FromStr;
use std::time::Duration;
use tokio::process::Command;

/// opener_id 白名单。未知值返回 Err。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenTargetId {
    Finder,
    Cursor,
    VsCode,
    Zed,
    Xcode,
    Ghostty,
    ITerm,
    Terminal,
    AndroidStudio,
    Antigravity,
}

impl FromStr for OpenTargetId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "finder" => Ok(Self::Finder),
            "cursor" => Ok(Self::Cursor),
            "vscode" => Ok(Self::VsCode),
            "zed" => Ok(Self::Zed),
            "xcode" => Ok(Self::Xcode),
            "ghostty" => Ok(Self::Ghostty),
            "iterm" => Ok(Self::ITerm),
            "terminal" => Ok(Self::Terminal),
            "android-studio" => Ok(Self::AndroidStudio),
            "antigravity" => Ok(Self::Antigravity),
            _ => Err(format!("Unknown opener: {s}")),
        }
    }
}

/// 单个应用的检测规格。
struct AppSpec {
    id: OpenTargetId,
    /// 前端显示用的 opener_id 字符串
    id_str: &'static str,
    label: &'static str,
    icon_name: &'static str,
    shortcut_key: Option<&'static str>,
    /// 用于 mdfind 的 .app 名称（如 "Visual Studio Code"）
    mdfind_name: &'static str,
    /// 用于 open -a 的名称
    open_name: &'static str,
}

impl AppSpec {
    /// Resolve spec from validated opener id (O(1), no iteration).
    fn from_id(id: OpenTargetId) -> &'static AppSpec {
        match id {
            OpenTargetId::Finder => &APP_SPECS[0],
            OpenTargetId::Cursor => &APP_SPECS[1],
            OpenTargetId::VsCode => &APP_SPECS[2],
            OpenTargetId::Zed => &APP_SPECS[3],
            OpenTargetId::Xcode => &APP_SPECS[4],
            OpenTargetId::Ghostty => &APP_SPECS[5],
            OpenTargetId::ITerm => &APP_SPECS[6],
            OpenTargetId::Terminal => &APP_SPECS[7],
            OpenTargetId::AndroidStudio => &APP_SPECS[8],
            OpenTargetId::Antigravity => &APP_SPECS[9],
        }
    }

    /// 构建 OpenTarget，available 需由调用方填充。
    fn to_target(&self, available: bool) -> OpenTarget {
        OpenTarget {
            id: self.id_str.to_string(),
            label: self.label.to_string(),
            icon_name: self.icon_name.to_string(),
            available,
            shortcut_key: self.shortcut_key.map(String::from),
        }
    }
}

/// 10 个 macOS 应用规格（对齐上游 openInLauncher.ts 的 TARGETS）。
static APP_SPECS: &[AppSpec] = &[
    AppSpec {
        id: OpenTargetId::Finder,
        id_str: "finder",
        label: "Finder",
        icon_name: "folder",
        shortcut_key: None,
        mdfind_name: "",
        open_name: "",
    },
    AppSpec {
        id: OpenTargetId::Cursor,
        id_str: "cursor",
        label: "Cursor",
        icon_name: "file-code",
        shortcut_key: Some("1"),
        mdfind_name: "Cursor",
        open_name: "Cursor",
    },
    AppSpec {
        id: OpenTargetId::VsCode,
        id_str: "vscode",
        label: "VS Code",
        icon_name: "file-code",
        shortcut_key: Some("\u{2318}O"),
        mdfind_name: "Visual Studio Code",
        open_name: "Visual Studio Code",
    },
    AppSpec {
        id: OpenTargetId::Zed,
        id_str: "zed",
        label: "Zed",
        icon_name: "square-code",
        shortcut_key: Some("2"),
        mdfind_name: "Zed",
        open_name: "Zed",
    },
    AppSpec {
        id: OpenTargetId::Xcode,
        id_str: "xcode",
        label: "Xcode",
        icon_name: "hammer",
        shortcut_key: Some("3"),
        mdfind_name: "Xcode",
        open_name: "Xcode",
    },
    AppSpec {
        id: OpenTargetId::Ghostty,
        id_str: "ghostty",
        label: "Ghostty",
        icon_name: "terminal",
        shortcut_key: Some("4"),
        mdfind_name: "Ghostty",
        open_name: "Ghostty",
    },
    AppSpec {
        id: OpenTargetId::ITerm,
        id_str: "iterm",
        label: "iTerm",
        icon_name: "terminal",
        shortcut_key: Some("5"),
        mdfind_name: "iTerm",
        open_name: "iTerm",
    },
    AppSpec {
        id: OpenTargetId::Terminal,
        id_str: "terminal",
        label: "Terminal",
        icon_name: "terminal",
        shortcut_key: Some("6"),
        mdfind_name: "Terminal",
        open_name: "Terminal",
    },
    AppSpec {
        id: OpenTargetId::AndroidStudio,
        id_str: "android-studio",
        label: "Android Studio",
        icon_name: "smartphone",
        shortcut_key: Some("7"),
        mdfind_name: "Android Studio",
        open_name: "Android Studio",
    },
    AppSpec {
        id: OpenTargetId::Antigravity,
        id_str: "antigravity",
        label: "Antigravity",
        icon_name: "square-code",
        shortcut_key: Some("8"),
        mdfind_name: "Antigravity",
        open_name: "Antigravity",
    },
];

/// 检测单个应用是否已安装。
async fn detect_single_app(spec: &AppSpec) -> bool {
    // Finder 始终可用
    if spec.id == OpenTargetId::Finder {
        return true;
    }

    // Terminal 检查固定系统路径
    if spec.id == OpenTargetId::Terminal {
        return tokio::fs::metadata("/System/Applications/Utilities/Terminal.app")
            .await
            .is_ok();
    }

    // mdfind 检测（Spotlight）
    let mdfind_ok = match Command::new("mdfind")
        .arg(format!("kMDItemFSName == '{}.app'", spec.mdfind_name))
        .output()
        .await
    {
        Ok(out) => out.status.success() && !out.stdout.is_empty(),
        Err(_) => false,
    };
    if mdfind_ok {
        return true;
    }

    // Fallback: 检查 /Applications/{name}.app
    tokio::fs::metadata(format!("/Applications/{}.app", spec.mdfind_name))
        .await
        .is_ok()
}

/// 检测所有已安装的外部应用，返回可用目标列表。
/// 非 macOS 平台返回空 Vec。
pub async fn detect_installations() -> Vec<OpenTarget> {
    if cfg!(not(target_os = "macos")) {
        return Vec::new();
    }

    let detections: Vec<_> = APP_SPECS
        .iter()
        .map(|spec| async move {
            match tokio::time::timeout(Duration::from_secs(2), detect_single_app(spec)).await {
                Ok(available) => spec.to_target(available),
                Err(_) => spec.to_target(false), // timeout = not available
            }
        })
        .collect();

    futures::future::join_all(detections)
        .await
        .into_iter()
        .filter(|t| t.available)
        .collect()
}

/// 用指定应用打开文件或目录。
/// 非 macOS 平台返回 Err(OpenFailed)。
pub async fn open_with(opener_id: &str, path: &str, is_directory: bool) -> Result<(), AppError> {
    if cfg!(not(target_os = "macos")) {
        return Err(AppError::OpenFailed(
            "Not supported on this platform".into(),
        ));
    }

    let target_id = OpenTargetId::from_str(opener_id).map_err(|e| AppError::InvalidInput(e))?;

    match target_id {
        OpenTargetId::Finder => {
            // Finder: open <path>（目录）或 open -R <path>（文件选中）
            let mut cmd = Command::new("open");
            if !is_directory {
                cmd.arg("-R");
            }
            cmd.arg(path);
            let output = cmd
                .output()
                .await
                .map_err(|e| AppError::OpenFailed(format!("Failed to open in Finder: {e}")))?;
            if !output.status.success() {
                return Err(AppError::OpenFailed("The path could not be opened".into()));
            }
        }
        _ => {
            // 其他应用: open -a "{name}" <path>
            let spec = AppSpec::from_id(target_id);
            let output = Command::new("open")
                .arg("-a")
                .arg(spec.open_name)
                .arg(path)
                .output()
                .await
                .map_err(|e| {
                    AppError::OpenFailed(format!("Failed to open in {}: {e}", spec.label))
                })?;
            if !output.status.success() {
                return Err(AppError::OpenFailed("The path could not be opened".into()));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_str_known_variants() {
        let cases = [
            ("finder", OpenTargetId::Finder),
            ("cursor", OpenTargetId::Cursor),
            ("vscode", OpenTargetId::VsCode),
            ("zed", OpenTargetId::Zed),
            ("xcode", OpenTargetId::Xcode),
            ("ghostty", OpenTargetId::Ghostty),
            ("iterm", OpenTargetId::ITerm),
            ("terminal", OpenTargetId::Terminal),
            ("android-studio", OpenTargetId::AndroidStudio),
            ("antigravity", OpenTargetId::Antigravity),
        ];
        for (input, expected) in cases {
            assert_eq!(
                OpenTargetId::from_str(input).unwrap(),
                expected,
                "Failed for: {input}"
            );
        }
    }

    #[test]
    fn from_str_rejects_unknown() {
        assert!(OpenTargetId::from_str("notepad").is_err());
        assert!(OpenTargetId::from_str("").is_err());
    }

    #[test]
    fn app_specs_count_matches() {
        assert_eq!(APP_SPECS.len(), 10);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn detect_installations_returns_finder() {
        let targets = detect_installations().await;
        assert!(
            targets.iter().any(|t| t.id == "finder"),
            "Finder should always be available on macOS"
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[tokio::test]
    async fn detect_installations_empty_on_non_macos() {
        let targets = detect_installations().await;
        assert!(targets.is_empty());
    }

    #[cfg(not(target_os = "macos"))]
    #[tokio::test]
    async fn open_with_fails_on_non_macos() {
        let result = open_with("finder", "/tmp", true).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Not supported"));
    }

    #[tokio::test]
    async fn open_with_rejects_invalid_opener() {
        let result = open_with("notepad", "/tmp", true).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Unknown opener"));
    }
}
