// prd-zen-mode-v1 Z29 — the Zen root's error boundary. A render error in the
// page (a widget, the template's controls) drops the window into safe mode
// with "The page crashed: <message>". If the built-in safe page itself
// throws, the boundary shows a last-resort panel whose Exit Zen is a plain
// button (the menu item and the chord work regardless).

import React from 'react'

interface Props {
  /** Called once with the error; the parent switches to safe mode. */
  onCrash(message: string): void
  /** True while rendering the safe page: a crash there can't fall back. */
  safe: boolean
  onExit(): void
  /** K2's safe-mode banner, kept on screen by the last-resort panel. */
  banner?: React.ReactNode
  children: React.ReactNode
}

interface State {
  error: Error | null
}

export class ZenErrorBoundary extends React.Component<Props, State> {
  state: State = { error: null }

  static getDerivedStateFromError(error: Error): State {
    return { error }
  }

  componentDidCatch(error: Error, info: React.ErrorInfo): void {
    console.error('[zen] the page crashed:', error, info.componentStack)
    if (!this.props.safe) this.props.onCrash(error.message || String(error))
  }

  render(): React.ReactNode {
    if (!this.state.error) return this.props.children
    if (!this.props.safe) return null
    return (
      <div className="flex h-full w-full flex-col">
        {this.props.banner}
        <div className="flex min-h-0 w-full flex-1 items-center justify-center" data-zen-last-resort="">
          <div className="text-center" style={{ color: 'var(--zen-text)' }}>
            <p>Zen can’t draw this window: {this.state.error.message}</p>
            <button
              type="button"
              className="mt-3 px-3 py-1"
              style={{ border: '1px solid var(--zen-border)', borderRadius: 'var(--zen-radius)' }}
              onClick={this.props.onExit}
            >
              Exit Zen
            </button>
          </div>
        </div>
      </div>
    )
  }
}
