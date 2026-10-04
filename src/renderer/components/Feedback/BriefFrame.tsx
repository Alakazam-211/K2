// Ticket HTML brief — the locked frame (prd-ticket-html-brief-v1 H12–H14,
// H18, H38, H39).
//
// The brief is untrusted agent HTML, already cleaned by the daemon. It is
// shown ONLY through the shared `HtmlFrame` with the `inert` profile:
// `sandbox=""` (no scripts, no same-origin, no popups, no forms, no top
// navigation), `referrerPolicy="no-referrer"`, and the per-frame CSP meta
// first in the document. Links are listed below the frame with their FULL
// URL and open in the system browser through plugin-opener (`openUrl`),
// never the in-app Browser tab. The list is compact: as tall as its rows,
// capped small with its own scroll, and absent when there are no links.
//
// Size: a fixed `min(60vh, 560px)` box with its own scrollbar and a drag
// handle; no auto-size (the parent can't read an inert frame's height).
// Expand shows the same document as a full-page overlay. Its header is a
// title bar like every page's top bar: the window inset, 38px tall, px-3,
// draggable, and on macOS it reserves the stoplight cluster before the
// title (the same spacer DesktopChromeLeft uses), so the lights never
// cover it. Linux and Windows keep the title at the left edge.
//
// The frame document is memoized on the brief's sha256 + the app theme, so
// the thread's event-driven refetches never reload the frame.

import React, { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react'
import { createPortal } from 'react-dom'
import { openUrl } from '@tauri-apps/plugin-opener'
import { HtmlFrame } from '@/components/HtmlFrame/HtmlFrame'
import {
  getDesktopChrome,
  TRAFFIC_LIGHT_CLUSTER_GAP_PX,
  TRAFFIC_LIGHT_SPACER_BASE_PX,
  type DesktopChrome,
} from '@/lib/desktop-chrome'
import { titleBarDragOnMouseDown, titleBarOnDoubleClick } from '@/lib/titlebar-drag'
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

/** Compact links list: as tall as its rows up to `BRIEF_LINKS_MAX_HEIGHT_CLASS`,
 *  then it scrolls. No links → nothing rendered (no space taken). */
export const BRIEF_LINKS_MAX_HEIGHT_CLASS = 'max-h-14'

function BriefLinks({ links }: { links: BriefLink[] }): React.JSX.Element | null {
  if (links.length === 0) return null
  return (
    <div data-testid="brief-links" className="mt-1 text-[11px] flex-none">
      <div className="text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)] leading-tight">
        Links in this brief
      </div>
      <ol data-testid="brief-links-list" className={`flex flex-col gap-0.5 ${BRIEF_LINKS_MAX_HEIGHT_CLASS} overflow-y-auto`}>
        {links.map((l) => (
          <li key={l.n} className="flex items-baseline gap-1.5 min-w-0 leading-tight">
            <span className="text-[var(--color-text-muted)] tabular-nums flex-shrink-0">[{l.n}]</span>
            <button
              type="button"
              data-testid="brief-link"
              onClick={() => openLink(l.url)}
              title={`Open in your browser: ${l.url}`}
              className="min-w-0 truncate text-left text-[var(--color-accent)] hover:underline cursor-pointer selectable-copy"
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
  expanded: expandedProp,
  onExpandedChange,
  hideToolbar = false,
  heightClass = DEFAULT_HEIGHT_CLASS,
}: {
  brief: FeedbackBrief
  /** The ticket title — names the frame and the Expand overlay. */
  title: string
  /** Controlled Expand overlay (the ticket detail header owns the button). */
  expanded?: boolean
  onExpandedChange?: (expanded: boolean) => void
  /** Hide the "Brief · HTML · size · Expand" row (the header has Expand). */
  hideToolbar?: boolean
  /** Box height until the user drags the handle (a Tailwind class). */
  heightClass?: string
}): React.JSX.Element {
  const themeKey = useBriefThemeKey()
  // Memo on sha256 + theme (H39): a thread refetch hands a new `brief`
  // object only if the brief itself changed, and briefs never change (H8).
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const built = useMemo(() => buildBriefSrcDoc(brief.html, readBriefTokens()), [brief.sha256, themeKey])

  const [height, setHeight] = useState<number | null>(null)
  const [dragging, setDragging] = useState(false)
  const [expandedLocal, setExpandedLocal] = useState(false)
  const expanded = expandedProp ?? expandedLocal
  const setExpanded = useCallback(
    (v: boolean) => {
      if (onExpandedChange) onExpandedChange(v)
      if (expandedProp === undefined) setExpandedLocal(v)
    },
    [onExpandedChange, expandedProp],
  )
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

  const close = useCallback(() => setExpanded(false), [setExpanded])

  return (
    <div data-testid="brief" className="mb-3">
      {!hideToolbar && (
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
      )}
      <div
        ref={boxRef}
        data-testid="brief-box"
        className={`relative border border-[var(--color-border)] ${height === null ? heightClass : ''}`}
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

/** Top bar height shared with the Feedback, Wiki, and Projects pages. */
const OVERLAY_TOPBAR_HEIGHT = 38

/**
 * Width reserved before the overlay title for the macOS stoplights: the
 * page top bars' spacer (`TRAFFIC_LIGHT_SPACER_BASE_PX`, measured from the
 * px-3 padding) plus their cluster gap. 0 on hosted web, Linux, Windows.
 * The native lights follow `--inset-window` (style.ts re-applies them on
 * resize, fullscreen, and display moves), and the overlay sits on the same
 * inset, so this holds in every case the page top bars hold. This is the
 * 100% value; the rendered spacer is `.k2-stoplight-spacer-brief`, which
 * follows `--k2-stoplight-spacer` under app zoom.
 */
export function briefOverlayStoplightInset(chrome: DesktopChrome): number {
  return chrome.trafficLightSpacer ? TRAFFIC_LIGHT_SPACER_BASE_PX + TRAFFIC_LIGHT_CLUSTER_GAP_PX : 0
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

  const stoplightInset = briefOverlayStoplightInset(getDesktopChrome())

  return createPortal(
    <div
      data-testid="brief-overlay"
      role="dialog"
      aria-modal="true"
      aria-label={`Brief: ${title}`}
      className="fixed inset-[var(--inset-window)] z-[400] flex flex-col bg-[var(--color-bg)]"
    >
      <div
        data-testid="brief-overlay-header"
        className="flex items-center gap-3 px-3 border-b border-[var(--color-border)] flex-shrink-0 select-none"
        style={{ height: OVERLAY_TOPBAR_HEIGHT, minHeight: OVERLAY_TOPBAR_HEIGHT }}
        onMouseDown={titleBarDragOnMouseDown}
        onDoubleClick={titleBarOnDoubleClick}
      >
        {stoplightInset > 0 && (
          <div
            data-testid="brief-overlay-stoplight-inset"
            aria-hidden
            // `stoplightInset - 12` at 100%; gap-3 (12) already separates
            // it from the title. The class follows the app zoom.
            className="k2-stoplight-spacer-brief"
          />
        )}
        <span
          data-testid="brief-overlay-title"
          className="no-drag min-w-0 text-sm font-medium text-[var(--color-text-primary)] truncate selectable-copy"
        >
          {title}
        </span>
        <button
          type="button"
          data-testid="brief-overlay-close"
          onClick={onClose}
          className="ml-auto flex-shrink-0 px-3 py-1 text-[11px] text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] transition-colors cursor-pointer"
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
        <div className="px-4 pb-2 border-t border-[var(--color-border)] flex-shrink-0">
          <BriefLinks links={links} />
        </div>
      )}
    </div>,
    document.body,
  )
}
