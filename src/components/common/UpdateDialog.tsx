/**
 * UpdateDialog - Modal dialog for app update flow.
 *
 * Three phases based on updateStatus:
 * 1. available: show version + release notes + download button
 * 2. downloading: progress bar (cannot cancel — API limitation)
 * 3. downloaded: restart button
 * 4. download-error: error message + retry/close buttons
 */

import { useEffect, useRef } from 'react';
import ReactMarkdown from 'react-markdown';

import { markdownComponents } from '@renderer/components/chat/markdownComponents';
import { useStore } from '@renderer/store';
import { CheckCircle, Loader2, X, AlertCircle } from 'lucide-react';
import remarkGfm from 'remark-gfm';

/**
 * Normalize release notes: strip HTML tags and convert block elements to newlines.
 * Uses DOMParser for proper HTML entity decoding (handles all entities like &mdash;, &#39;, etc.)
 */
function normalizeReleaseNotes(html: string): string {
  if (!html?.trim()) return '';

  const processed = html
    .replace(/<\/p>\s*/gi, '\n\n')
    .replace(/<br\s*\/?>\s*/gi, '\n')
    .replace(/<\/div>\s*/gi, '\n')
    .replace(/<\/li>\s*/gi, '\n')
    .replace(/<\/h[1-6]>\s*/gi, '\n\n');

  const parser = new DOMParser();
  const doc = parser.parseFromString(processed, 'text/html');
  const text = doc.body.textContent || '';

  return text.replace(/\n{3,}/g, '\n\n').trim();
}

export const UpdateDialog = (): React.JSX.Element | null => {
  const showUpdateDialog = useStore((s) => s.showUpdateDialog);
  const updateStatus = useStore((s) => s.updateStatus);
  const availableVersion = useStore((s) => s.availableVersion);
  const releaseNotes = useStore((s) => s.releaseNotes);
  const downloadProgress = useStore((s) => s.downloadProgress);
  const downloadError = useStore((s) => s.downloadError);
  const downloadUpdate = useStore((s) => s.downloadUpdate);
  const installAndRestart = useStore((s) => s.installAndRestart);
  const retryDownload = useStore((s) => s.retryDownload);
  const dismissUpdateDialog = useStore((s) => s.dismissUpdateDialog);

  const dialogRef = useRef<HTMLDivElement>(null);

  const canDismiss = updateStatus === 'available' || updateStatus === 'download-error';

  // Handle ESC key to close dialog (only when dismissable)
  useEffect(() => {
    if (!showUpdateDialog || !canDismiss) return;

    const handleEscape = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        dismissUpdateDialog();
      }
    };

    document.addEventListener('keydown', handleEscape);
    return () => document.removeEventListener('keydown', handleEscape);
  }, [showUpdateDialog, canDismiss, dismissUpdateDialog]);

  // Focus trap
  useEffect(() => {
    if (!showUpdateDialog || !dialogRef.current) return;

    const dialog = dialogRef.current;
    const focusableElements = dialog.querySelectorAll<HTMLElement>(
      'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])',
    );
    const firstElement = focusableElements[0];
    const lastElement = focusableElements[focusableElements.length - 1];

    firstElement?.focus();

    const handleTab = (e: KeyboardEvent): void => {
      if (e.key !== 'Tab') return;

      if (e.shiftKey) {
        if (document.activeElement === firstElement) {
          e.preventDefault();
          lastElement?.focus();
        }
      } else {
        if (document.activeElement === lastElement) {
          e.preventDefault();
          firstElement?.focus();
        }
      }
    };

    dialog.addEventListener('keydown', handleTab);
    return () => dialog.removeEventListener('keydown', handleTab);
  }, [showUpdateDialog, updateStatus]);

  if (!showUpdateDialog) return null;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center">
      {/* Backdrop — only clickable when dismissable */}
      {canDismiss && (
        <button
          className="absolute inset-0 cursor-default"
          style={{ backgroundColor: 'rgba(0, 0, 0, 0.6)' }}
          onClick={dismissUpdateDialog}
          aria-label="Close dialog"
          tabIndex={-1}
        />
      )}
      {!canDismiss && (
        <div className="absolute inset-0" style={{ backgroundColor: 'rgba(0, 0, 0, 0.6)' }} />
      )}

      <div
        ref={dialogRef}
        className="relative mx-4 w-full max-w-sm rounded-md border p-4 shadow-lg"
        role="dialog"
        aria-modal="true"
        aria-label="Update available"
        style={{
          backgroundColor: 'var(--color-surface-overlay)',
          borderColor: 'var(--color-border-emphasis)',
        }}
      >
        {/* Close button — only when dismissable */}
        {canDismiss && (
          <button
            onClick={dismissUpdateDialog}
            className="absolute right-3 top-3 rounded p-1 transition-colors hover:bg-white/10"
            style={{ color: 'var(--color-text-muted)' }}
          >
            <X className="size-4" />
          </button>
        )}

        {/* Phase 1: Update available */}
        {updateStatus === 'available' && (
          <>
            <div className="mb-3 pr-8">
              <h2 className="text-base font-semibold" style={{ color: 'var(--color-text)' }}>
                Update Available
              </h2>
              {availableVersion && (
                <div className="mt-1 text-xs" style={{ color: 'var(--color-text-secondary)' }}>
                  v{availableVersion}
                </div>
              )}
            </div>

            {releaseNotes && (
              <div
                className="prose prose-sm mb-4 max-h-48 overflow-y-auto rounded border p-2 text-xs"
                style={{
                  backgroundColor: 'var(--color-surface)',
                  borderColor: 'var(--color-border)',
                  color: 'var(--color-text-muted)',
                }}
              >
                <ReactMarkdown remarkPlugins={[remarkGfm]} components={markdownComponents}>
                  {normalizeReleaseNotes(releaseNotes)}
                </ReactMarkdown>
              </div>
            )}

            <div className="flex justify-end gap-2">
              <button
                onClick={dismissUpdateDialog}
                className="rounded-md border px-3 py-1.5 text-sm font-medium transition-colors hover:bg-white/5"
                style={{
                  borderColor: 'var(--color-border)',
                  color: 'var(--color-text-secondary)',
                }}
              >
                Later
              </button>
              <button
                onClick={downloadUpdate}
                className="rounded-md bg-blue-600 px-3 py-1.5 text-sm font-medium text-white transition-colors hover:bg-blue-500"
              >
                下载更新
              </button>
            </div>
          </>
        )}

        {/* Phase 2: Downloading */}
        {updateStatus === 'downloading' && (
          <>
            <div className="mb-3">
              <h2 className="text-base font-semibold" style={{ color: 'var(--color-text)' }}>
                正在下载更新...
              </h2>
              {availableVersion && (
                <div className="mt-1 text-xs" style={{ color: 'var(--color-text-secondary)' }}>
                  v{availableVersion}
                </div>
              )}
            </div>

            {/* Progress bar */}
            <div className="mb-3">
              <div className="mb-1.5 flex items-center justify-between text-xs" style={{ color: 'var(--color-text-secondary)' }}>
                <div className="flex items-center gap-1.5">
                  <Loader2 className="size-3 animate-spin" />
                  <span>Downloading</span>
                </div>
                <span>{downloadProgress}%</span>
              </div>
              <div className="h-2 w-full overflow-hidden rounded-full" style={{ backgroundColor: 'var(--color-surface)' }}>
                <div
                  className="h-full rounded-full bg-blue-600 transition-all duration-300 ease-out"
                  style={{ width: `${downloadProgress}%` }}
                />
              </div>
            </div>
          </>
        )}

        {/* Phase 3a: Downloaded */}
        {updateStatus === 'downloaded' && (
          <>
            <div className="mb-3">
              <h2 className="text-base font-semibold" style={{ color: 'var(--color-text)' }}>
                更新已就绪
              </h2>
              {availableVersion && (
                <div className="mt-1 text-xs" style={{ color: 'var(--color-text-secondary)' }}>
                  v{availableVersion}
                </div>
              )}
            </div>

            <div className="mb-4 flex items-center gap-2 text-sm" style={{ color: 'var(--color-text)' }}>
              <CheckCircle className="size-4 text-green-500" />
              <span>更新已下载完成，点击下方按钮重启应用以完成安装。</span>
            </div>

            <div className="flex justify-end">
              <button
                onClick={installAndRestart}
                className="rounded-md bg-blue-600 px-3 py-1.5 text-sm font-medium text-white transition-colors hover:bg-blue-500"
              >
                重启并安装
              </button>
            </div>
          </>
        )}

        {/* Phase 3b: Download error */}
        {updateStatus === 'download-error' && (
          <>
            <div className="mb-3">
              <h2 className="text-base font-semibold" style={{ color: 'var(--color-text)' }}>
                下载失败
              </h2>
            </div>

            <div className="mb-4 flex items-start gap-2 rounded border p-2 text-sm" style={{ backgroundColor: 'var(--color-surface)', borderColor: 'var(--color-border)' }}>
              <AlertCircle className="mt-0.5 size-4 shrink-0 text-red-400" />
              <span style={{ color: 'var(--color-text-muted)' }}>
                {downloadError ?? 'Unknown download error'}
              </span>
            </div>

            <div className="flex justify-end gap-2">
              <button
                onClick={dismissUpdateDialog}
                className="rounded-md border px-3 py-1.5 text-sm font-medium transition-colors hover:bg-white/5"
                style={{
                  borderColor: 'var(--color-border)',
                  color: 'var(--color-text-secondary)',
                }}
              >
                关闭
              </button>
              <button
                onClick={retryDownload}
                className="rounded-md bg-blue-600 px-3 py-1.5 text-sm font-medium text-white transition-colors hover:bg-blue-500"
              >
                重试
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
};
