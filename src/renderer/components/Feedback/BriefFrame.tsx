// Ticket HTML brief — the locked frame (prd-ticket-html-brief-v1 H12–H14,
// H18, H38, H39).
//
// The brief is untrusted agent HTML, already cleaned by the daemon. It is
// shown ONLY through the shared `HtmlFrame` with the `inert` profile:
// `sandbox=""` (no scripts, no same-origin, no popups, no forms, no top
// navigation), `referrerPolicy="no-referrer"`, and the per-frame CSP meta
// first in the document. Links are listed below the frame with their FULL
// URL and open in the system browser through plugin-opener (`openUrl`),
// never the in-app Browser tab.
//
// Size: a fixed `min(60vh, 560px)` box with its own scrollbar and a drag
// handle; no auto-size (the parent can't read an inert frame's height).
// Expand shows the same document as a full-page overlay.
//
// The frame document is memoized on the brief's sha256 + the app theme, so
// the thread's event-driven refetches never reload the frame.

import React, { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react'
import { createPortal } from 'react-dom'
import { openUrl } from '@tauri-apps/plugin-opener'
import { HtmlFrame } from '@/components/HtmlFrame/HtmlFrame'
import type { FeedbackBrief } from './feedback-api'
import { buildBriefSrcDoc, readBriefTokens, type BriefLink } from './brief-srcdoc'

const THEME_ATTRS = ['data-scheme', 'data-palette', 'data-style']

function subscribeTheme(onChange: () => void): () => void {
  const obs = new MutationObserver(onChange)
  obs.observe(document.documentElement, { attributes: true, attributeFilter: THEME_ATTRS })
  return () => obs.disconnect()
}

function themeSnapshot(): string {
  const el = document.documentElement
  return THEME_ATTRS.map((a) => el.getAttribute(a) ?? '').join('|')
}

/** A key that changes when the app scheme/palette/style changes, so the
 *  brief document is rebuilt with the new tokens (H14). */
export function useBriefThemeKey(): string {
  return useSyncExternalStore(subscribeTheme, themeSnapshot, themeSnapshot)
}

/** Default frame height `min(60vh, 560px)` (H18): a class until the user
 *  drags the handle, then an inline px height. */
const DEFAULT_HEIGHT_CLASS = 'h-[min(60vh,560px)]'
const MIN_HEIGHT = 120
const KEY_STEP = 32

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  return `${(n / (1024 * 1024)).toFixed(2)} MB`
}

function openLink(url: string): void {
  // System browser (plugin-opener allows http/https/mailto). Deliberately
  // NOT openOffOriginHttp, which would open the in-app Browser tab (H13).
  openUrl(url).catch((e: unknown) => console.warn('[brief] openUrl failed', url, e))
}

function BriefLinks({ links }: { links: BriefLink[] }): React.JSX.Element | null {
  if (links.length === 0) return null
  return (
    <div data-testid="brief-links" className="mt-2 text-[11px]">
      <div className="text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)] mb-1">
        Links in this brief
      </div>
      <ol className="flex flex-col gap-0.5">
        {links.map((l) => (
          <li key={l.n} className="flex items-baseline gap-1.5 min-w-0">
            <span className="text-[var(--color-text-muted)] tabular-nums flex-shrink-0">[{l.n}]</span>
            <button
              type="button"
              data-testid="brief-link"
              onClick={() => openLink(l.url)}
              title="Open in your browser"
              className="text-left text-[var(--color-accent)] hover:underline break-all cursor-pointer selectable-copy"
            >
              {l.url}
            </button>
          </li>
        ))}
      </ol>
    </div>
  )
}

export function BriefFrame({
  brief,
  title,
}: {
  brief: FeedbackBrief
  /** The ticket title — names the frame and the Expand overlay. */
  title: string
}): React.JSX.Element {
  const themeKey = useBriefThemeKey()
  // Memo on sha256 + theme (H39): a thread refetch hands a new `brief`
  // object only if the brief itself changed, and briefs never change (H8).
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const built = useMemo(() => buildBriefSrcDoc(brief.html, readBriefTokens()), [brief.sha256, themeKey])

  const [height, setHeight] = useState<number | null>(null)
  const [dragging, setDragging] = useState(false)
  const [expanded, setExpanded] = useState(false)
  const boxRef = useRef<HTMLDivElement>(null)

  const clamp = (h: number): number =>
    Math.max(MIN_HEIGHT, Math.min(h, Math.round(window.innerHeight * 0.9)))

  const onHandlePointerDown = (e: React.PointerEvent<HTMLDivElement>): void => {
    e.preventDefault()
    const box = boxRef.current
    if (!box) return
    const startY = e.clientY
    const startH = box.getBoundingClientRect().height
    setDragging(true)
    const move = (ev: PointerEvent): void => setHeight(clamp(startH + ev.clientY - startY))
    const up = (): void => {
      setDragging(false)
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', up)
    }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', up)
  }

  const onHandleKey = (e: React.KeyboardEvent<HTMLDivElement>): void => {
    if (e.key !== 'ArrowUp' && e.key !== 'ArrowDown') return
    e.preventDefault()
    const current = boxRef.current?.getBoundingClientRect().height ?? 400
    setHeight(clamp(current + (e.key === 'ArrowDown' ? KEY_STEP : -KEY_STEP)))
  }

  const close = useCallback(() => setExpanded(false), [])

  return (
    <div data-testid="brief" className="mb-3">
      <div className="flex items-center gap-2 mb-1">
        <span className="text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)]">
          Brief
        </span>
        <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums opacity-70">
          HTML · {formatBytes(brief.bytes)}
        </span>
        <button
          type="button"
          data-testid="brief-expand"
          onClick={() => setExpanded(true)}
          className="ml-auto px-2 py-0.5 text-[10px] text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors cursor-pointer"
        >
          Expand
        </button>
      </div>
      <div
        ref={boxRef}
        data-testid="brief-box"
        className={`relative border border-[var(--color-border)] ${height === null ? DEFAULT_HEIGHT_CLASS : ''}`}
        style={height === null ? undefined : { height: `${height}px` }}
      >
        <HtmlFrame
          html={built.frameHtml}
          profile="inert"
          title={`Brief: ${title}`}
          className={`block w-full h-full border-0 ${dragging ? 'pointer-events-none' : ''}`}
          testId="brief-frame"
        />
      </div>
      <div
        role="separator"
        aria-orientation="horizontal"
        aria-label="Resize brief"
        tabIndex={0}
        data-testid="brief-resize"
        onPointerDown={onHandlePointerDown}
        onKeyDown={onHandleKey}
        className="h-2 cursor-row-resize flex items-center justify-center group"
      >
        <div className="w-10 h-0.5 bg-[var(--color-border)] group-hover:bg-[var(--color-text-muted)]" />
      </div>
      <BriefLinks links={built.links} />
      {expanded && (
        <BriefOverlay title={title} frameHtml={built.frameHtml} links={built.links} onClose={close} />
      )}
    </div>
  )
}

function BriefOverlay({
  title,
  frameHtml,
  links,
  onClose,
}: {
  title: string
  frameHtml: string
  links: BriefLink[]
  onClose: () => void
}): React.JSX.Element {
  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.stopPropagation()
        onClose()
      }
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [onClose])

  return createPortal(
    <div
      data-testid="brief-overlay"
      role="dialog"
      aria-modal="true"
      aria-label={`Brief: ${title}`}
      className="fixed inset-0 z-[400] flex flex-col bg-[var(--color-bg)]"
    >
      <div className="flex items-center gap-3 px-4 py-2 border-b border-[var(--color-border)] flex-shrink-0">
        <span className="text-sm font-medium text-[var(--color-text-primary)] truncate flex-1 selectable-copy">
          {title}
        </span>
        <button
          type="button"
          data-testid="brief-overlay-close"
          onClick={onClose}
          className="px-3 py-1 text-[11px] text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] transition-colors cursor-pointer"
        >
          Close
        </button>
      </div>
      <div className="flex-1 min-h-0">
        <HtmlFrame
          html={frameHtml}
          profile="inert"
          title={`Brief: ${title}`}
          className="block w-full h-full border-0"
          testId="brief-overlay-frame"
        />
      </div>
      {links.length > 0 && (
        <div className="px-4 py-2 border-t border-[var(--color-border)] max-h-[30vh] overflow-y-auto flex-shrink-0">
          <BriefLinks links={links} />
        </div>
      )}
    </div>,
    document.body,
  )
}
