import { useEffect, useState, useCallback, useMemo } from 'react'
import { daemonCliGet } from '@/lib/daemon-cli'
import Markdown from '../Markdown/Markdown'

interface WhatsNewPayload {
  current_version: string
  last_seen_version: string | null
  has_new: boolean
  content: string
}

interface VersionPage {
  version: string
  title: string
  body: string
}

interface WhatsNewModalProps {
  /**
   * - `auto` (default): runs `whats_new_check` on mount; opens the
   *   popup if `has_new: true`. Used in the main + focus-mode layouts
   *   so the popup fires automatically on first launch after every
   *   K2 update.
   * - `button-only`: skips the mount-time check. Stays dormant until
   *   the Settings "Read what's new" button dispatches
   *   `k2so:show-whats-new`. Used in the Settings layout so opening
   *   Settings never auto-fires the popup.
   */
  mode?: 'auto' | 'button-only'
}

/**
 * 0.38.7 — User-facing changelog popup with per-version pagination.
 * 0.38.11 — split into two trigger modes so the Settings-mounted
 * instance doesn't auto-fire on Settings open.
 *
 * Mounts at the app root. On mount in `auto` mode, asks the daemon
 * whether there's something the user hasn't seen yet (via
 * `whats_new_check`). If yes, shows a modal — one version per page,
 * newest first. The user can navigate through every version they
 * skipped. "Got it" marks the current daemon version as seen; the
 * modal won't reopen until the next K2 update.
 *
 * Daemon-side logic lives in `k2so_core::whats_new`. This component
 * does client-side splitting of the markdown into per-version pages
 * for the pagination UI, but the source of truth for what to show is
 * the daemon's `slice_unseen` output.
 */
export default function WhatsNewModal({
  mode = 'auto'
}: WhatsNewModalProps = {}): React.JSX.Element | null {
  const [payload, setPayload] = useState<WhatsNewPayload | null>(null)
  const [visible, setVisible] = useState(false)
  const [dismissing, setDismissing] = useState(false)
  const [pageIdx, setPageIdx] = useState(0)

  // Shared check function — used by initial mount AND by the
  // `k2so:show-whats-new` event from Settings → Release notes button.
  // The event-driven path force-opens regardless of `has_new` (the
  // Settings button just reset state, so has_new should be true; but
  // even if a race leaves it false, we want to surface SOMETHING when
  // the user explicitly asks to re-read).
  //
  // 0.38.13: Rust-side `whats_new_check` is now single-shot. If the
  // daemon is briefly unreachable (launch race, kickstart-restart
  // in progress, etc.), retry here in JS instead of blocking a
  // Tauri worker thread. `setTimeout` yields control between
  // attempts so the rest of the renderer can keep painting.
  const runCheck = useCallback(async (forceShow: boolean) => {
    const MAX_ATTEMPTS = 10
    const BACKOFF_MS = 500
    let attempt = 0
    while (attempt < MAX_ATTEMPTS) {
      try {
        const data = await daemonCliGet<WhatsNewPayload>('whats_new')
        setPayload(data)
        if (data.has_new || forceShow) {
          setVisible(true)
        }
        return
      } catch (err) {
        attempt += 1
        if (attempt >= MAX_ATTEMPTS) {
          // eslint-disable-next-line no-console
          console.debug('[whats-new] check failed after retries:', err)
          return
        }
        await new Promise((r) => setTimeout(r, BACKOFF_MS))
      }
    }
  }, [])

  // Initial check on mount — `auto` mode only. The Settings instance
  // (mode='button-only') stays dormant until the Read-what's-new
  // button dispatches `k2so:show-whats-new`.
  //
  // 0.38.13: deferred by 2s so the popup check doesn't compete with
  // workspace hydration, daemon version-check, and other boot-time
  // invokes — improves perceived launch speed. After 2s the main
  // UI has already painted, so the popup appears as a clear "after"
  // event rather than a boot stampede contributor.
  useEffect(() => {
    if (mode !== 'auto') return
    const id = setTimeout(() => {
      void runCheck(false)
    }, 2000)
    return () => clearTimeout(id)
  }, [mode, runCheck])

  // Listen for the "Read what's new" button in Settings. Resets daemon
  // state then dispatches this event; we re-check and force-open.
  useEffect(() => {
    const handler = (): void => {
      void runCheck(true)
    }
    window.addEventListener('k2so:show-whats-new', handler)
    return () => window.removeEventListener('k2so:show-whats-new', handler)
  }, [runCheck])

  // Split the joined markdown into one entry per version. The daemon
  // returns them newest-first (the order they appear in WHATS_NEW.md);
  // we reverse so page 0 = oldest unseen, last page = newest. That
  // way forward (→) walks chronologically toward the current version
  // — "catch up on what you missed, in order."
  const pages: VersionPage[] = useMemo(() => {
    if (!payload?.content) return []
    const out: VersionPage[] = []
    const lines = payload.content.split('\n')
    let current: { version: string; title: string; body: string[] } | null = null
    for (const line of lines) {
      const m = line.match(/^## (\d+\.\d+\.\d+)\s+[—-]\s+(.+?)\s*$/)
      if (m) {
        if (current) out.push({ ...current, body: current.body.join('\n') })
        current = { version: m[1], title: m[2], body: [] }
      } else if (current) {
        current.body.push(line)
      }
    }
    if (current) out.push({ ...current, body: current.body.join('\n') })
    // Reverse: oldest first → newest last. Forward arrow = newer.
    out.reverse()
    return out
  }, [payload?.content])

  // Default landing page is the NEWEST version (the one the user just
  // updated to). They can navigate back (←) to read older releases
  // they may have skipped. Effect fires once when pages first
  // populates — user-driven navigation doesn't trigger it because
  // the `pages` array reference is stable.
  useEffect(() => {
    if (pages.length > 0) {
      setPageIdx(pages.length - 1)
    }
  }, [pages])

  const totalPages = pages.length
  const currentPage = pages[pageIdx]

  const handleDismiss = useCallback(async () => {
    if (dismissing) return
    setDismissing(true)
    try {
      await daemonCliGet('whats_new/mark_seen')
    } catch (err) {
      // eslint-disable-next-line no-console
      console.debug('[whats-new] mark_seen failed:', err)
    }
    setVisible(false)
    setDismissing(false)
  }, [dismissing])

  const goPrev = useCallback(() => {
    setPageIdx((idx) => Math.max(0, idx - 1))
  }, [])
  const goNext = useCallback(() => {
    setPageIdx((idx) => Math.min(totalPages - 1, idx + 1))
  }, [totalPages])

  // Keyboard: Esc dismisses; ←/→ paginate.
  useEffect(() => {
    if (!visible) return
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.preventDefault()
        e.stopPropagation()
        void handleDismiss()
      } else if (e.key === 'ArrowLeft') {
        e.preventDefault()
        goPrev()
      } else if (e.key === 'ArrowRight') {
        e.preventDefault()
        goNext()
      }
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [visible, handleDismiss, goPrev, goNext])

  if (!visible || !payload || !currentPage) return null

  const isFirst = pageIdx === 0
  const isLast = pageIdx === totalPages - 1

  return (
    <>
      {/* Scoped styles for the markdown body. */}
      <style>{`
        .whats-new-body {
          font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text',
            'Inter', system-ui, 'Segoe UI', sans-serif;
          font-size: 13.5px;
          line-height: 1.6;
          color: var(--color-text-primary);
        }
        .whats-new-body .wn-title {
          font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Display',
            'Inter', system-ui, sans-serif;
          font-size: 17px;
          font-weight: 600;
          color: var(--color-text-primary);
          margin: 2px 0 14px;
          line-height: 1.3;
        }
        .whats-new-body p {
          margin: 0 0 12px;
        }
        .whats-new-body ul {
          margin: 4px 0 12px;
          padding-left: 22px;
          list-style: disc;
        }
        .whats-new-body li {
          margin: 3px 0;
        }
        .whats-new-body code {
          font-family: 'MesloLGM Nerd Font', Menlo, Monaco, monospace;
          font-size: 12px;
          background: var(--color-bg-subtle, rgba(255,255,255,0.06));
          padding: 1px 5px;
          color: var(--color-text-primary);
        }
        .whats-new-body strong { font-weight: 600; }
        .whats-new-body em { font-style: italic; opacity: 0.92; }
        .whats-new-body a {
          color: var(--color-accent, #4a9eff);
          text-decoration: underline;
        }
        .wn-nav-btn {
          background: transparent;
          border: 1px solid var(--color-border);
          color: var(--color-text-primary);
          font-family: -apple-system, BlinkMacSystemFont, 'SF Pro Text',
            'Inter', system-ui, sans-serif;
          font-size: 13px;
          padding: 5px 12px;
          cursor: pointer;
          transition: opacity 0.12s ease;
        }
        .wn-nav-btn:disabled {
          opacity: 0.35;
          cursor: not-allowed;
        }
        .wn-nav-btn:not(:disabled):hover {
          background: color-mix(in srgb, var(--color-text-primary) 6%, transparent);
        }
      `}</style>

      {/* Backdrop. */}
      <div
        style={{
          position: 'fixed',
          inset: 0,
          zIndex: 99998,
          background: 'rgba(0, 0, 0, 0.55)'
        }}
        onMouseDown={(e) => {
          e.stopPropagation()
          void handleDismiss()
        }}
      />

      {/* Dialog */}
      <div
        className="no-drag"
        style={{
          position: 'fixed',
          top: '50%',
          left: '50%',
          transform: 'translate(-50%, -50%)',
          zIndex: 99999,
          width: 'min(620px, 90vw)',
          maxHeight: '78vh',
          display: 'flex',
          flexDirection: 'column',
          background: 'var(--color-bg-surface)',
          border: '1px solid var(--color-border)',
          boxShadow:
            '0 12px 40px rgba(0, 0, 0, 0.6), 0 2px 8px rgba(0, 0, 0, 0.4)',
          overflow: 'hidden'
        }}
        onMouseDown={(e) => e.stopPropagation()}
      >
        {/* Header — overall title + per-page version chip + page-count */}
        <div
          style={{
            padding: '14px 22px',
            borderBottom: '1px solid var(--color-border)',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'space-between',
            gap: 12,
            background:
              'linear-gradient(180deg, color-mix(in srgb, var(--color-accent, #4a9eff) 6%, transparent) 0%, transparent 100%)',
            fontFamily:
              "-apple-system, BlinkMacSystemFont, 'SF Pro Display', 'Inter', system-ui, sans-serif"
          }}
        >
          <div
            style={{
              fontSize: '15px',
              fontWeight: 600,
              color: 'var(--color-text-primary)'
            }}
          >
            What's new in K2
          </div>

          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 10
            }}
          >
            {/* Version chip — the version of the CURRENT page */}
            <span
              style={{
                fontFamily:
                  "'MesloLGM Nerd Font', Menlo, Monaco, monospace",
                fontSize: '12px',
                fontWeight: 500,
                color: 'var(--color-accent, #4a9eff)',
                background:
                  'color-mix(in srgb, var(--color-accent, #4a9eff) 14%, transparent)',
                padding: '3px 9px'
              }}
            >
              v{currentPage.version}
            </span>

            {/* Page-count badge (only when there's more than one) */}
            {totalPages > 1 && (
              <span
                style={{
                  fontSize: '11.5px',
                  color: 'var(--color-text-secondary)',
                  fontFamily:
                    "'MesloLGM Nerd Font', Menlo, Monaco, monospace"
                }}
              >
                {pageIdx + 1} / {totalPages}
              </span>
            )}
          </div>
        </div>

        {/* Body — single version, sans-serif typography */}
        <div
          className="whats-new-body"
          style={{
            padding: '16px 22px 18px',
            overflowY: 'auto',
            flex: 1
          }}
        >
          <div className="wn-title">{currentPage.title}</div>
          <Markdown>{currentPage.body}</Markdown>
        </div>

        {/* Footer — pagination controls + dismiss */}
        <div
          style={{
            padding: '12px 22px 14px',
            borderTop: '1px solid var(--color-border)',
            display: 'flex',
            justifyContent: 'space-between',
            alignItems: 'center',
            gap: 8,
            background: 'var(--color-bg-surface)',
            fontFamily:
              "-apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Inter', system-ui, sans-serif"
          }}
        >
          {/* Pagination only shows when there's more than one page */}
          {totalPages > 1 ? (
            <div style={{ display: 'flex', gap: 6 }}>
              <button
                className="wn-nav-btn"
                onClick={(e) => {
                  e.stopPropagation()
                  goPrev()
                }}
                disabled={isFirst}
                title="Previous version (←)"
              >
                ←
              </button>
              <button
                className="wn-nav-btn"
                onClick={(e) => {
                  e.stopPropagation()
                  goNext()
                }}
                disabled={isLast}
                title="Next version (→)"
              >
                →
              </button>
            </div>
          ) : (
            <span />
          )}

          <button
            onClick={(e) => {
              e.stopPropagation()
              void handleDismiss()
            }}
            disabled={dismissing}
            style={{
              padding: '7px 22px',
              fontSize: '13px',
              fontFamily: 'inherit',
              fontWeight: 500,
              border: '1px solid var(--color-accent, #4a9eff)',
              background: 'var(--color-accent, #4a9eff)',
              color: '#ffffff',
              cursor: dismissing ? 'wait' : 'pointer',
              opacity: dismissing ? 0.6 : 1,
              lineHeight: '1.4'
            }}
          >
            Got it
          </button>
        </div>
      </div>
    </>
  )
}
