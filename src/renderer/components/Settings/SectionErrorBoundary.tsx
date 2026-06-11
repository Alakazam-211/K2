import React from 'react'

export class SectionErrorBoundary extends React.Component<
  { children: React.ReactNode },
  { error: Error | null }
> {
  state: { error: Error | null } = { error: null }

  static getDerivedStateFromError(error: Error): { error: Error } {
    return { error }
  }

  componentDidCatch(error: Error, info: React.ErrorInfo): void {
    console.error('[Settings] Section render error:', error, info.componentStack)
  }

  render(): React.ReactNode {
    if (this.state.error) {
      return (
        <div className="max-w-xl p-4">
          <h2 className="text-sm font-medium text-red-400 mb-2">Something went wrong</h2>
          <p className="text-xs text-[var(--color-text-muted)] mb-3">
            This section failed to render. Try restarting the app.
          </p>
          <pre className="text-[10px] text-red-400/70 bg-red-500/5 border border-red-500/20 p-2 overflow-x-auto whitespace-pre-wrap">
            {this.state.error.message}
          </pre>
          <button
            onClick={() => this.setState({ error: null })}
            className="mt-3 px-3 py-1 text-xs text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)] no-drag cursor-pointer"
          >
            Try Again
          </button>
        </div>
      )
    }
    return this.props.children
  }
}
