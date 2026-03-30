/**
 * AdvancedSection - Advanced settings including config management and about info.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { api, isDesktopMode } from "@renderer/api";
import appIcon from "@renderer/favicon.png";
import { useStore } from "@renderer/store";
import {
  CheckCircle,
  Code2,
  Download,
  Loader2,
  RefreshCw,
  Upload,
} from "lucide-react";

import { SettingsSectionHeader } from "../components";

interface AdvancedSectionProps {
  readonly saving: boolean;
  readonly onResetToDefaults: () => void;
  readonly onExportConfig: () => void;
  readonly onImportConfig: () => void;
  readonly onOpenInEditor: () => void;
}

export const AdvancedSection = ({
  saving,
  onResetToDefaults,
  onExportConfig,
  onImportConfig,
  onOpenInEditor,
}: AdvancedSectionProps): React.JSX.Element => {
  const isDesktop = useMemo(() => isDesktopMode(), []);
  const [version, setVersion] = useState<string>("");
  const updateStatus = useStore((s) => s.updateStatus);
  const availableVersion = useStore((s) => s.availableVersion);
  const checkForUpdates = useStore((s) => s.checkForUpdates);

  // Auto-revert "not-available" / "error" status back to idle after a brief display
  const revertTimerRef = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => {
    if (updateStatus === "not-available" || updateStatus === "error") {
      revertTimerRef.current = setTimeout(() => {
        useStore.setState({ updateStatus: "idle" });
      }, 3000);
    }
    return () => {
      if (revertTimerRef.current) clearTimeout(revertTimerRef.current);
    };
  }, [updateStatus]);

  useEffect(() => {
    api.getAppVersion().then(setVersion).catch(console.error);
  }, []);

  const handleCheckForUpdates = useCallback(() => {
    checkForUpdates();
  }, [checkForUpdates]);

  const getUpdateButtonContent = (): React.JSX.Element => {
    switch (updateStatus) {
      case "checking":
        return (
          <>
            <Loader2 className="size-3.5 animate-spin" />
            Checking...
          </>
        );
      case "not-available":
        return (
          <>
            <CheckCircle className="size-3.5" />
            Up to date
          </>
        );
      case "available":
      case "downloaded":
        return (
          <>
            <Download className="size-3.5" />
            {updateStatus === "downloaded"
              ? "Update ready"
              : `v${availableVersion ?? "unknown"} available`}
          </>
        );
      default:
        return (
          <>
            <RefreshCw className="size-3.5" />
            Check for Updates
          </>
        );
    }
  };

  return (
    <div>
      <SettingsSectionHeader title="Configuration" />
      <div className="space-y-2 py-2">
        <button
          onClick={onResetToDefaults}
          disabled={saving}
          className={`flex w-full items-center justify-center gap-2 rounded-md border px-4 py-2.5 text-sm font-medium transition-all duration-150 ${saving ? "cursor-not-allowed opacity-50" : ""} `}
          style={{
            borderColor: "var(--color-border)",
            color: "var(--color-text-secondary)",
          }}
        >
          <RefreshCw className="size-4" />
          Reset to Defaults
        </button>
        <button
          onClick={onExportConfig}
          disabled={saving}
          className={`flex w-full items-center justify-center gap-2 rounded-md border px-4 py-2.5 text-sm font-medium transition-all duration-150 ${saving ? "cursor-not-allowed opacity-50" : ""} `}
          style={{
            borderColor: "var(--color-border)",
            color: "var(--color-text-secondary)",
          }}
        >
          <Download className="size-4" />
          Export Config
        </button>
        <button
          onClick={onImportConfig}
          disabled={saving}
          className={`flex w-full items-center justify-center gap-2 rounded-md border px-4 py-2.5 text-sm font-medium transition-all duration-150 ${saving ? "cursor-not-allowed opacity-50" : ""} `}
          style={{
            borderColor: "var(--color-border)",
            color: "var(--color-text-secondary)",
          }}
        >
          <Upload className="size-4" />
          Import Config
        </button>
        {isDesktop && (
          <button
            onClick={onOpenInEditor}
            className="flex w-full items-center justify-center gap-2 rounded-md border px-4 py-2.5 text-sm font-medium transition-all duration-150"
            style={{
              borderColor: "var(--color-border)",
              color: "var(--color-text-secondary)",
            }}
          >
            <Code2 className="size-4" />
            Open in Editor
          </button>
        )}
      </div>

      <SettingsSectionHeader title="About" />
      <div className="flex items-start gap-4 py-3">
        <img src={appIcon} alt="App Icon" className="size-10 rounded-lg" />
        <div>
          <div className="flex items-center gap-3">
            <p
              className="text-sm font-medium"
              style={{ color: "var(--color-text)" }}
            >
              claude-devtools
            </p>
            {isDesktop && (
              <button
                onClick={handleCheckForUpdates}
                disabled={updateStatus === "checking"}
                className="flex items-center gap-1.5 rounded-md border px-2.5 py-1 text-xs font-medium transition-colors hover:bg-white/5 disabled:opacity-50"
                style={{
                  borderColor: "var(--color-border)",
                  color:
                    updateStatus === "not-available"
                      ? "var(--color-text-muted)"
                      : updateStatus === "available" ||
                          updateStatus === "downloaded"
                        ? "#60a5fa"
                        : "var(--color-text-secondary)",
                }}
              >
                {getUpdateButtonContent()}
              </button>
            )}
            {!isDesktop && (
              <span
                className="rounded-md border px-2.5 py-1 text-xs font-medium"
                style={{
                  borderColor: "var(--color-border)",
                  color: "var(--color-text-muted)",
                }}
              >
                Standalone
              </span>
            )}
          </div>
          <p
            className="mt-0.5 text-xs"
            style={{ color: "var(--color-text-muted)" }}
          >
            Version {version || "..."}
          </p>
          <p
            className="mt-2 text-xs leading-relaxed"
            style={{ color: "var(--color-text-muted)" }}
          >
            Visualize and analyze Claude Code session executions with
            interactive waterfall charts and detailed insights.
          </p>
          <div className="mt-2 flex items-center gap-2 text-xs" style={{ color: "var(--color-text-muted)" }}>
            <span>Made by Vino</span>
            <a
              href="https://github.com/augustVino/claude-devtools-tauri"
              target="_blank"
              rel="noopener noreferrer"
              className="inline-flex items-center gap-1 transition-colors hover:text-[var(--color-text-secondary)]"
            >
              <svg className="size-3" viewBox="0 0 16 16" fill="currentColor"><path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z"/></svg>
              GitHub
            </a>
          </div>
        </div>
      </div>
    </div>
  );
};
