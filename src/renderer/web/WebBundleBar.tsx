/**
 * Hosted web: the small non-blocking bar for a web client version change
 * (see ./bundle-version). "K2 was updated · Reload" when the server moved
 * to a newer published bundle while this tab was open; a plain note when
 * the server's bundle isn't published. Clicks and keys fall through
 * everywhere except the bar's own buttons.
 */

import React from 'react'
import { reloadOntoOfferedBundle, useWebBundleBar, type BundleBar } from './bundle-version'

/** The bar's words (exported for tests). */
export function webBundleBarText(bar: BundleBar): string {
  if (bar.kind === 'updated') return `K2 was updated to ${bar.version}.`
  return `This server runs K2 ${bar.version}, but its web client isn't published yet. You're on ${bar.bundleVersion || 'an older client'}, so some things may not work.`
}

const buttonStyle: React.CSSProperties = {
  padding: '0.15rem 0.6rem',
  fontSize: '0.75rem',
  borderRadius: 0,
  border: '1px solid var(--color-border, rgba(255,255,255,0.25))',
  cursor: 'pointer',
}

export function WebBundleBar(): React.ReactElement | null {
  const bar = useWebBundleBar((s) => s.bar)
  const show = useWebBundleBar((s) => s.show)
  if (!bar) return null
  return <WebBundleBarView bar={bar} onDismiss={() => show(null)} />
}

/** The bar itself, for one state (exported for tests). */
export function WebBundleBarView({
  bar,
  onDismiss,
}: {
  bar: BundleBar
  onDismiss: () => void
}): React.ReactElement {
  return (
    <div
      role="status"
      aria-live="polite"
      data-testid="web-bundle-bar"
      style={{
        position: 'fixed',
        left: 0,
        right: 0,
        // Above the RecoveryBanner pill (bottom 1.25rem) so both can show.
        bottom: '3.75rem',
        zIndex: 9999,
        display: 'flex',
        justifyContent: 'center',
        pointerEvents: 'none',
      }}
    >
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: '0.5rem',
          maxWidth: 'min(40rem, calc(100vw - 2rem))',
          padding: '0.4rem 0.85rem',
          borderRadius: 'var(--radius-box, 0px)',
          background: 'rgba(20,20,20,0.92)',
          border: '1px solid var(--color-border, rgba(255,255,255,0.12))',
          boxShadow: '0 2px 12px rgba(0,0,0,0.35)',
          color: 'var(--color-text-primary, #e0e0e0)',
          fontFamily: 'system-ui, -apple-system, sans-serif',
          fontSize: '0.8rem',
          pointerEvents: 'auto',
        }}
      >
        <span>{webBundleBarText(bar)}</span>
        {bar.kind === 'updated' && (
          <button
            type="button"
            onClick={() => reloadOntoOfferedBundle(bar.version, bar.via)}
            style={{ ...buttonStyle, background: 'var(--color-accent, #2f6feb)', color: 'var(--color-on-accent)' }}
          >
            Reload
          </button>
        )}
        <button
          type="button"
          aria-label="Dismiss"
          onClick={onDismiss}
          style={{ ...buttonStyle, background: 'transparent', color: 'inherit' }}
        >
          ×
        </button>
      </div>
    </div>
  )
}
