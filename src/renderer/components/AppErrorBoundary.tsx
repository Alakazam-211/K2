import React from 'react'

/**
 * Top-level error boundary around the whole <App> tree.
 *
 * Without this, an error thrown while React re-mounts the main layout — most
 * notably on EXITING Settings (`settingsOpen` flips false and App re-renders
 * the full Sidebar/TerminalArea/panels/dialogs tree) — propagates to the React
 * root and unmounts EVERYTHING, leaving a black screen recoverable only by
 * right-click → Reload or relaunching the app. (Root cause of the reported
 * "black screen after hitting Back from Settings".)
 *
 * This degrades that to a recoverable error panel with a Reload button, and
 * logs the error + component stack so the underlying transient throw (a pane
 * re-attaching to its PTY, a store selector momentarily undefined, a WS event
 * mid-remount, …) can be pinned and hardened. App is keyed by host, so only
 * focus mode had a boundary before — this covers the Settings + main-layout
 * branches too.
 */
export class AppErrorBoundary extends React.Component<
  { children: React.ReactNode },
  { error: Error | null }
> {
  state: { error: Error | null } = { error: null }

  static getDerivedStateFromError(error: Error): { error: Error } {
    return { error }
  }

  componentDidCatch(error: Error, info: React.ErrorInfo): void {
    console.error('[AppErrorBoundary] CRASH:', error, info.componentStack)
  }

  render(): React.ReactNode {
    if (this.state.error) {
      return (
        <div className="flex h-full w-full items-center justify-center bg-[var(--color-bg)] p-8 no-drag">
          <div className="max-w-lg text-xs">
            <p className="font-bold text-[var(--color-status-error-soft)] mb-2">Something went wrong rendering K2.</p>
            <p className="text-[var(--color-text-muted)] mb-3">
              The view hit an unexpected error. Reloading usually fixes it — your work and
              workspaces are safe.
            </p>
            <button
              onClick={() => window.location.reload()}
              className="px-3 py-1.5 text-xs font-medium text-[var(--color-accent)] border border-[var(--color-accent)]/40 hover:bg-[var(--color-accent)]/10 transition-colors cursor-pointer no-drag rounded-none"
            >
              Reload
            </button>
            <pre className="whitespace-pre-wrap text-[var(--color-text-muted)] mt-3 max-h-40 overflow-auto">
              {this.state.error.message}
            </pre>
          </div>
        </div>
      )
    }
    return this.props.children
  }
}
