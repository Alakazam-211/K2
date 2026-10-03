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
 *
 * The panel's collapsible Details section shows the same component stack and
 * copies it, so a crash report names the component without opening devtools.
 */
interface AppErrorBoundaryState {
  error: Error | null
  /** React's component stack from componentDidCatch — names the component
   *  that threw, which `error.message` alone never does. */
  componentStack: string | null
  copied: boolean
}

/** The text the Details "Copy" button puts on the clipboard. */
export function crashReportText(error: Error, componentStack: string | null): string {
  return [
    error.message,
    error.stack ? `\nStack:\n${error.stack}` : '',
    componentStack ? `\nComponent stack:${componentStack}` : '',
  ].join('')
}

export class AppErrorBoundary extends React.Component<
  { children: React.ReactNode },
  AppErrorBoundaryState
> {
  state: AppErrorBoundaryState = { error: null, componentStack: null, copied: false }

  static getDerivedStateFromError(error: Error): Partial<AppErrorBoundaryState> {
    return { error }
  }

  componentDidCatch(error: Error, info: React.ErrorInfo): void {
    const componentStack = info.componentStack ?? null
    console.error('[AppErrorBoundary] CRASH:', error, '\nComponent stack:', componentStack)
    this.setState({ componentStack })
  }

  private copyDetails = (): void => {
    const { error, componentStack } = this.state
    if (!error) return
    void navigator.clipboard
      ?.writeText(crashReportText(error, componentStack))
      .then(() => this.setState({ copied: true }))
      .catch((e) => console.error('[AppErrorBoundary] copy failed:', e))
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
            <details className="mt-3 text-[var(--color-text-muted)]">
              <summary className="cursor-pointer select-none no-drag">Details</summary>
              <pre
                data-testid="app-error-component-stack"
                className="whitespace-pre-wrap mt-2 max-h-60 overflow-auto select-text"
              >
                {this.state.componentStack?.trim() || 'No component stack was reported.'}
              </pre>
              <button
                onClick={this.copyDetails}
                className="mt-2 px-3 py-1.5 text-xs font-medium text-[var(--color-accent)] border border-[var(--color-accent)]/40 hover:bg-[var(--color-accent)]/10 transition-colors cursor-pointer no-drag rounded-none"
              >
                {this.state.copied ? 'Copied' : 'Copy details'}
              </button>
            </details>
          </div>
        </div>
      )
    }
    return this.props.children
  }
}
