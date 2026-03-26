// Copyright 2025 Claude DevTools Contributors
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tauri::{command, AppHandle, Emitter};
use tauri_plugin_updater::{Update, UpdaterExt};

/// Holds a pending update (after check, before download/install).
pub struct PendingUpdate(pub Mutex<Option<Update>>);

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
/// If an update is found, it is stored in `PendingUpdate` state for later download.
#[command]
pub async fn check_for_updates(
  app: AppHandle,
  state: tauri::State<'_, PendingUpdate>,
) -> Result<(), String> {
  let _ = app.emit("updater:status", &UpdaterStatus::Checking);

  let updater = app
    .updater_builder()
    .build()
    .map_err(|e| e.to_string())?;

  match updater.check().await {
    Ok(Some(update)) => {
      let version = update.version.clone();
      if let Ok(mut pending) = state.0.lock() {
        *pending = Some(update);
      }
      let _ = app.emit(
        "updater:status",
        &UpdaterStatus::Available { version },
      );
    }
    Ok(None) => {
      let _ = app.emit("updater:status", &UpdaterStatus::UpToDate);
    }
    Err(e) => {
      let _ = app.emit(
        "updater:status",
        &UpdaterStatus::Error {
          message: e.to_string(),
        },
      );
    }
  }

  Ok(())
}

/// Download and install the pending update.
///
/// Requires a prior call to `check_for_updates`.
/// Emits download progress events, then installs the update.
#[command]
pub async fn download_and_install_update(
  app: AppHandle,
  state: tauri::State<'_, PendingUpdate>,
) -> Result<(), String> {
  let update = state
    .0
    .lock()
    .map_err(|e| e.to_string())?
    .take()
    .ok_or("No update available. Call check_for_updates first.")?;

  let _ = app.emit(
    "updater:status",
    &UpdaterStatus::Downloading {
      progress: 0.0,
      content_length: None,
    },
  );

  let app_handle = app.clone();
  update
    .download_and_install(
      |chunk_length, content_length| {
        let progress = content_length
          .map(|total| (chunk_length as f64 / total as f64) * 100.0)
          .unwrap_or(0.0);
        let _ = app_handle.emit(
          "updater:status",
          &UpdaterStatus::Downloading {
            progress,
            content_length,
          },
        );
      },
      || {
        let _ = app_handle.emit("updater:status", &UpdaterStatus::Downloaded);
      },
    )
    .await
    .map_err(|e| e.to_string())?;

  Ok(())
}

/// Request an application restart to apply the installed update.
#[command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
  app.request_restart();
  Ok(())
}
