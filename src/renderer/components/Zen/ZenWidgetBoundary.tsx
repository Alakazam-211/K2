// prd-zen-user-widgets-v2 UW31 [UW55] — every widget gets its own error
// boundary, built-ins included.
//
// A widget that throws while rendering shows "This widget crashed:
// <message>. [Reload]" in its own box; the page and the other widgets keep
// running. Safe mode (Z29) keeps its other triggers: a crash in K2's
// chrome (the template's controls are not wrapped here), a failed
// required-controls check, an unreachable daemon, or Shift. The page-level
// `ZenErrorBoundary` stays the outer net.
//
// The boundary draws nothing of its own until a widget crashes, so a page
// that never crashes looks exactly as before.

import React from 'react'

interface Props {
  /** The placement id (logged, and on the card for tests). */
  widgetId: string
  kind: string
  children: React.ReactNode
}

interface State {
  error: Error | null
  /** Bumped by Reload: remounts the widget from scratch. */
  generation: number
}

/** The card's text (exported for tests and the custom widget's own cards). */
export function zenWidgetCrashText(message: string): string {
  const clean = message.replace(/[.\s]+$/, '') || 'unknown error'
  return `This widget crashed: ${clean}.`
}

export class ZenWidgetBoundary extends React.Component<Props, State> {
  state: State = { error: null, generation: 0 }

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error }
  }

  componentDidCatch(error: Error, info: React.ErrorInfo): void {
    console.error(`[zen] widget ${this.props.widgetId} (${this.props.kind}) crashed:`, error, info.componentStack)
  }

  private reload = (): void => {
    this.setState((s) => ({ error: null, generation: s.generation + 1 }))
  }

  render(): React.ReactNode {
    const { error, generation } = this.state
    if (!error) return <React.Fragment key={generation}>{this.props.children}</React.Fragment>
    return (
      <div
        role="alert"
        data-zen-widget-crashed={this.props.widgetId}
        data-zen-widget={this.props.kind}
        className="flex h-full w-full flex-col items-center justify-center text-center"
        style={{ padding: 16, gap: 10, color: 'var(--zen-text-muted)', minHeight: 0, flex: '1 1 0%' }}
      >
        <span style={{ color: 'var(--zen-text)' }}>{zenWidgetCrashText(error.message || String(error))}</span>
        <button
          type="button"
          data-zen-widget-reload=""
          data-zen-soft-button=""
          onClick={this.reload}
          className="cursor-pointer"
          style={{
            padding: '4px 12px',
            border: '1px solid var(--zen-border)',
            borderRadius: 'calc(var(--zen-radius) - 4px)',
            color: 'var(--zen-text)',
            background: 'transparent',
          }}
        >
          Reload
        </button>
      </div>
    )
  }
}
