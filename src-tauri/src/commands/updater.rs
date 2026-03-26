// Copyright 2025 Claude DevTools Contributors
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};
use tauri::{command, AppHandle, Emitter};

const UPDATER_NOT_CONFIGURED: &str = "Updater is not configured. Add plugins.updater to tauri.conf.json with pubkey and endpoints.";

/// Status events emitted on the `updater:status` channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum UpdaterStatus {
  /// Update check is in progress.
  Checking,
  /// A newer version is available.
  Available { version: String },
  /// Download is in progress with progress information.
  Downloading {
    progress: f64,
    content_length: Option<u64>,
  },
  /// Download completed; ready to install.
  Downloaded,
  /// Application is already up-to-date.
  UpToDate,
  /// An error occurred during the update process.
  Error { message: String },
}

/// Check for available updates.
///
/// Emits `UpdaterStatus` events on the `updater:status` channel.
#[command]
pub async fn check_for_updates(app: AppHandle) -> Result<(), String> {
  let _ = app.emit("updater:status", &UpdaterStatus::Checking);
  let _ = app.emit(
    "updater:status",
    &UpdaterStatus::Error {
      message: UPDATER_NOT_CONFIGURED.to_string(),
    },
  );
  Err(UPDATER_NOT_CONFIGURED.to_string())
}

/// Download and install the pending update.
#[command]
pub async fn download_and_install_update(app: AppHandle) -> Result<(), String> {
  Err(UPDATER_NOT_CONFIGURED.to_string())
}

/// Request an application restart to apply the installed update.
#[command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
  app.request_restart();
  Ok(())
}
