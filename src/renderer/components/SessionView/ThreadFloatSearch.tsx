import { useCallback, useEffect, useLayoutEffect, useRef, useState, type JSX, type KeyboardEvent } from 'react'
import { isMacPlatform } from '@/lib/desktop-chrome'
import { floatProps } from './threadFloats'
import { SPLIT_BAR_ATTR } from './splitChrome'
import {
  closeThreadSearch,
  currentMatchIndex,
  stepMatchIndex,
  threadSearchHost,
  threadSearchStatus,
  updateThreadSearch,
  useThreadSearchState,
} from './threadSearch'

/** Focus here keeps ⌘F / Ctrl+F: the terminal (Ctrl+F is a shell's
 *  forward-char) and code editors with their own find. */
const KEEPS_FIND_KEY = '[data-terminal-container], .cm-editor, .monaco-editor, [contenteditable="true"]'

/** On the floating header (or split view's bar) while the field is open and
 *  the row is short of room: `narrow` or `tight` (globals.css, "Thread
 *  view's search", "Split view's bar"). */
export const THREAD_SEARCHING_ATTR = 'data-thread-searching'
/** At or under this header width the field has the row to itself. */
export const THREAD_SEARCH_TIGHT_PX = 380

/** The find shortcut for this platform: ⌘F on a Mac, Ctrl+F elsewhere. */
export function isThreadFindKey(e: Pick<globalThis.KeyboardEvent, 'key' | 'metaKey' | 'ctrlKey' | 'altKey' | 'shiftKey'>, mac: boolean): boolean {
  if (e.key.toLowerCase() !== 'f' || e.altKey || e.shiftKey) return false
  return mac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey
}

function SearchGlyph({ size = 14 }: { size?: number }): JSX.Element {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" className="flex-shrink-0">
      <circle cx="11" cy="11" r="8" />
      <line x1="21" y1="21" x2="16.65" y2="16.65" />
    </svg>
  )
}

function Chevron({ up }: { up: boolean }): JSX.Element {
  return (
    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d={up ? 'M18 15l-6-6-6 6' : 'M6 9l6 6 6-6'} />
    </svg>
  )
}

function CloseGlyph(): JSX.Element {
  return (
    <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2.2} strokeLinecap="round" aria-hidden="true">
      <path d="M18 6L6 18M6 6l12 12" />
    </svg>
  )
}

const INNER_BUTTON =
  'inline-flex items-center justify-center h-5 w-5 flex-shrink-0 rounded-[var(--radius-field)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] disabled:opacity-40 disabled:pointer-events-none focus-visible:outline focus-visible:outline-2 focus-visible:outline-[var(--color-accent)]'

/** Split view's bar: the column it searches, and what that column shows. */
export interface SplitBarSearchTarget {
  host: HTMLElement
  kind: 'thread' | 'chatter'
}

/** A plain bar button, as restart's (not a floating object). */
const BAR_BUTTON =
  'self-center inline-flex items-center justify-center h-5 w-5 flex-shrink-0 rounded-[var(--radius-field)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] transition-colors no-drag cursor-pointer'

/** Thread view's search object, right of the bell: a round button that
 *  opens a field in the same row. The Thread below finds and highlights
 *  (useThreadSearchPane); this owns the field, the count and the keys.
 *
 *  `bar`: on split view's bar, searching that column. The button is a plain
 *  bar button, the field opens in the bar, and ⌘F / Ctrl+F opens it from
 *  anywhere in the split pane except the terminal. */
export function ThreadFloatSearch({ bar }: { bar?: SplitBarSearchTarget | null } = {}): JSX.Element {
  const rootRef = useRef<HTMLDivElement>(null)
  const inputRef = useRef<HTMLInputElement>(null)
  const buttonRef = useRef<HTMLButtonElement>(null)
  const barHost = bar?.host ?? null
  const resolveHost = useCallback(() => barHost ?? threadSearchHost(rootRef.current), [barHost])
  const what = bar?.kind === 'chatter' ? 'the Chatter' : 'this Thread'
  const s = useThreadSearchState(resolveHost)
  const mac = isMacPlatform()
  const total = s.matches.length
  const index = currentMatchIndex(s.matches, s.current)
  const status = threadSearchStatus({ query: s.query, index, total, searching: s.searching })
  // Which way focus goes when it closes: back to the button only when the
  // field had it (a click elsewhere keeps the click's focus).
  const [refocus, setRefocus] = useState(false)

  const open = useCallback(() => {
    const host = resolveHost()
    updateThreadSearch(host, (prev) => ({ open: true, focusTick: prev.focusTick + 1 }))
  }, [resolveHost])
  const close = useCallback(
    (toButton: boolean) => {
      setRefocus(toButton)
      closeThreadSearch(resolveHost())
    },
    [resolveHost],
  )
  const step = useCallback(
    (dir: 1 | -1) => {
      const host = resolveHost()
      updateThreadSearch(host, (prev) => {
        const i = stepMatchIndex(currentMatchIndex(prev.matches, prev.current), prev.matches.length, dir)
        return i < 0 ? null : { current: prev.matches[i].key }
      })
    },
    [resolveHost],
  )

  // Opened (or asked again): the field, with what was typed selected.
  useEffect(() => {
    if (!s.open) return
    const input = inputRef.current
    input?.focus()
    input?.select()
  }, [s.open, s.focusTick])

  useEffect(() => {
    if (s.open || !refocus) return
    setRefocus(false)
    buttonRef.current?.focus()
  }, [s.open, refocus])

  // ⌘F / Ctrl+F while focus is in this pane's Thread (its list, the
  // floating objects, the message box), never in its terminal or an editor.
  // On split view's bar: anywhere in the split pane (the bar's parent holds
  // the bar and both columns), except the terminal.
  useEffect(() => {
    const host = barHost ? rootRef.current?.closest(`[${SPLIT_BAR_ATTR}]`)?.parentElement : resolveHost()
    if (!host) return
    const onKey = (e: globalThis.KeyboardEvent): void => {
      if (!isThreadFindKey(e, mac)) return
      const t = e.target
      if (t instanceof Element && t.closest(KEEPS_FIND_KEY)) return
      e.preventDefault()
      e.stopPropagation()
      open()
    }
    host.addEventListener('keydown', onKey)
    return () => host.removeEventListener('keydown', onKey)
  }, [resolveHost, barHost, mac, open])

  // A click anywhere else closes it.
  useEffect(() => {
    if (!s.open) return
    const onDown = (e: MouseEvent): void => {
      const node = e.target
      if (node instanceof Node && rootRef.current?.contains(node)) return
      close(false)
    }
    document.addEventListener('mousedown', onDown)
    return () => document.removeEventListener('mousedown', onDown)
  }, [s.open, close])

  // Room for the field: where the row would overflow, the name and the
  // right-hand objects step aside while searching ("narrow"); in a very
  // narrow pane the field has the row to itself ("tight"). Measured with
  // the attribute off, before paint, so it can't flip back and forth. On
  // split view's bar, `narrow` hides the name and the other right-hand
  // objects, and if the row still overflows, `tight` leaves the field alone
  // in the bar.
  const inBar = barHost !== null
  useLayoutEffect(() => {
    const header = rootRef.current?.closest<HTMLElement>(inBar ? `[${SPLIT_BAR_ATTR}]` : '[data-thread-floats]')
    if (!header || !s.open) return
    const fit = (): void => {
      header.removeAttribute(THREAD_SEARCHING_ATTR)
      const width = header.clientWidth
      if (width <= 0) return
      if (width <= THREAD_SEARCH_TIGHT_PX) header.setAttribute(THREAD_SEARCHING_ATTR, 'tight')
      else if (header.scrollWidth > width + 1) {
        header.setAttribute(THREAD_SEARCHING_ATTR, 'narrow')
        if (inBar && header.scrollWidth > width + 1) header.setAttribute(THREAD_SEARCHING_ATTR, 'tight')
      }
    }
    fit()
    const ro = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(fit)
    ro?.observe(header)
    return () => {
      ro?.disconnect()
      header.removeAttribute(THREAD_SEARCHING_ATTR)
    }
  }, [s.open, inBar])

  // Closed elsewhere (the pane left Thread view): nothing stays painted.
  useEffect(() => () => closeThreadSearch(resolveHost()), [resolveHost])

  const onKeyDown = (e: KeyboardEvent<HTMLElement>): void => {
    if (e.key === 'Escape') {
      e.preventDefault()
      e.stopPropagation()
      close(true)
      return
    }
    if (e.key === 'Enter' && e.target === inputRef.current) {
      e.preventDefault()
      step(e.shiftKey ? -1 : 1)
    }
  }

  const shortcut = mac ? '⌘F' : 'Ctrl+F'
  return (
    <div ref={rootRef} className="flex min-w-0 items-center" data-testid="thread-search">
      {s.open ? (
        <div
          role="search"
          aria-label={`Search ${what}`}
          data-thread-search=""
          {...floatProps(true, 'pill')}
          className="gap-1 pl-2 pr-1"
          onKeyDown={onKeyDown}
        >
          <span className="text-[var(--color-text-muted)] inline-flex"><SearchGlyph size={12} /></span>
          <input
            ref={inputRef}
            type="search"
            data-testid="thread-search-input"
            aria-label={`Search messages in ${what}`}
            aria-keyshortcuts="Enter Shift+Enter Escape"
            placeholder="Search"
            autoComplete="off"
            autoCorrect="off"
            spellCheck={false}
            enterKeyHint="search"
            value={s.query}
            onChange={(e) => updateThreadSearch(resolveHost(), { query: e.target.value, current: null })}
            className="thread-search-input min-w-0 bg-transparent border-0 p-0 outline-none text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)]"
          />
          <span
            role="status"
            aria-live="polite"
            aria-atomic="true"
            data-testid="thread-search-count"
            data-empty={status.text === 'No matches' ? '' : undefined}
            className="thread-search-count flex-shrink-0 tabular-nums text-[var(--color-text-muted)] whitespace-nowrap"
          >
            <span aria-hidden="true">{status.text}</span>
            <span className="sr-only">{status.spoken}</span>
          </span>
          <button type="button" className={INNER_BUTTON} aria-label="Older match" title="Older match (Enter)" disabled={total === 0} onClick={() => step(1)} data-testid="thread-search-older">
            <Chevron up />
          </button>
          <button type="button" className={INNER_BUTTON} aria-label="Newer match" title="Newer match (Shift+Enter)" disabled={total === 0} onClick={() => step(-1)} data-testid="thread-search-newer">
            <Chevron up={false} />
          </button>
          <button type="button" className={INNER_BUTTON} aria-label="Close search" title="Close search (Esc)" onClick={() => close(true)} data-testid="thread-search-close">
            <CloseGlyph />
          </button>
        </div>
      ) : (
        <button
          ref={buttonRef}
          type="button"
          {...floatProps(!inBar, 'round')}
          onClick={open}
          aria-label={`Search ${what}`}
          aria-keyshortcuts={mac ? 'Meta+F' : 'Control+F'}
          title={`Search ${what} (${shortcut})`}
          data-testid="thread-search-button"
          className={inBar ? BAR_BUTTON : 'flex-shrink-0 no-drag cursor-pointer text-[var(--color-text-primary)]'}
        >
          <SearchGlyph />
        </button>
      )}
    </div>
  )
}
