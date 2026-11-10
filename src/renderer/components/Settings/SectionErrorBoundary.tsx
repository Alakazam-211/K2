import React from 'react'
import { Button } from '@/components/ui'

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
          <h2 className="type-title text-[var(--color-status-error-soft)] mb-2">Something went wrong</h2>
          <p className="type-hint text-[var(--color-text-secondary)] mb-3">
            This section failed to render. Try restarting the app.
          </p>
          <pre className="type-hint text-[var(--color-status-error-soft)] bg-[color-mix(in_srgb,var(--color-status-error)_5%,transparent)] border border-[color-mix(in_srgb,var(--color-status-error)_20%,transparent)] p-2 overflow-x-auto whitespace-pre-wrap">
            {this.state.error.message}
          </pre>
          <Button variant="quiet" className="mt-3" onClick={() => this.setState({ error: null })}>
            Try again
          </Button>
        </div>
      )
    }
    return this.props.children
  }
}
