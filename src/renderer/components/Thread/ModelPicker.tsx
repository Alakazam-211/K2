// The model picker (0.46.13; K2 Design's spec, thread-model-picker-design):
// the Thread composer's `/model` (and its model chip) and Settings →
// Providers / Models "Default model". The reaction picker's menu popover:
// the app's menu material (Frost in Glass, the cut surface in Terminal),
// 320px wide, at most 60% of the window's height, the list scrolling inside a ScrollEdgeFrame.
//
// A listbox: the search ("Find a model") keeps focus and names the active
// row with aria-activedescendant; ↑↓ move, Enter picks, Esc closes and gives
// focus back. The checked row carries a check glyph (never colour alone).

import { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type JSX } from 'react'
import { createPortal } from 'react-dom'
import { SearchField } from '@/components/ui/SearchField'
import { MENU_LIST, MENU_ROW, floatMaterial } from '@/components/ui/float-material'
import { ScrollEdgeFrame } from '@/components/ui/ScrollEdgeFade'
import { clientRectToCss, cssViewport } from '@/lib/app-zoom'
import { useBrowserOverlay } from '@/lib/browser-overlay'
import type { ModelRow } from '@/lib/model-choices'

export const MODEL_PICKER_WIDTH = 320
const ROW_PX = 44

/** The words for the picker's states (exported for tests). */
export const MODEL_PICKER_WORDS = {
  search: 'Find a model',
  loading: 'Loading models…',
  error: 'Couldn’t load models',
  retry: 'Retry',
  noMatch: (q: string) => `No model matches ‘${q}’.`,
  threadFooter: 'This chat only, from your next message.',
  settingFooter: 'Every new chat on this server starts with this model.',
} as const

/** Where a key moves the active row (exported for tests). */
export function modelPickerMove(active: number, key: string, count: number): number | null {
  if (count === 0) return null
  if (key === 'ArrowDown') return Math.min(active + 1, count - 1)
  if (key === 'ArrowUp') return Math.max(active - 1, 0)
  if (key === 'Home') return 0
  if (key === 'End') return count - 1
  return null
}

export interface ModelPickerProps {
  /** What it opens beside: above it, its left edge (below when no room). */
  anchor: HTMLElement | null
  /** All rows for the current search (the caller filters with `query`). */
  rows: (query: string) => ModelRow[]
  /** The checked row's value ('' = the default row). */
  checked: string
  state: 'loading' | 'error' | 'ready'
  onRetry?: () => void
  /** The search text it opens with (`/model op` → `op`). */
  initialQuery?: string
  footer: string
  label: string
  onPick: (value: string) => void
  /** `refocus`: Esc or a pick (give focus back), not a press outside. */
  onClose: (refocus: boolean) => void
  testId?: string
}

export function ModelPicker({
  anchor,
  rows,
  checked,
  state,
  onRetry,
  initialQuery = '',
  footer,
  label,
  onPick,
  onClose,
  testId = 'model-picker',
}: ModelPickerProps): JSX.Element {
  const [q, setQ] = useState(initialQuery)
  const list = useMemo(() => (state === 'ready' ? rows(q) : []), [rows, q, state])
  const [active, setActive] = useState(0)
  const ref = useRef<HTMLDivElement | null>(null)
  const searchRef = useRef<HTMLInputElement | null>(null)
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null)
  const base = useId()
  const optionId = (i: number): string => `${base}-opt-${i}`
  useBrowserOverlay({ active: true, ref, kind: 'popover', watch: pos })

  // The checked row starts active; a new search starts at the top.
  useEffect(() => {
    const at = list.findIndex((r) => r.value === checked)
    setActive(q.trim() || at < 0 ? 0 : at)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [q, state])

  useLayoutEffect(() => {
    const box = ref.current
    if (!box) return
    const { width: vw, height: vh } = cssViewport()
    const w = box.offsetWidth
    const h = box.offsetHeight
    const a = anchor ? clientRectToCss(anchor.getBoundingClientRect()) : { top: vh / 2, bottom: vh / 2, left: vw / 2, right: vw / 2 }
    let top = a.top - h - 6
    if (top < 8) top = Math.min(a.bottom + 6, vh - h - 8)
    const left = Math.max(8, Math.min(a.left, vw - w - 8))
    setPos({ left: Math.round(left), top: Math.round(Math.max(8, top)) })
  }, [anchor, list.length, state])

  const close = useCallback((refocus: boolean) => onClose(refocus), [onClose])

  useEffect(() => {
    searchRef.current?.focus()
    const onDown = (e: MouseEvent): void => {
      if (!(e.target instanceof Node)) return
      if (ref.current?.contains(e.target) || anchor?.contains(e.target)) return
      close(false)
    }
    document.addEventListener('mousedown', onDown)
    return () => document.removeEventListener('mousedown', onDown)
  }, [close, anchor])

  useEffect(() => {
    document.getElementById(optionId(active))?.scrollIntoView?.({ block: 'nearest' })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active])

  const pick = (r: ModelRow | undefined): void => {
    if (!r) return
    onPick(r.value)
    close(true)
  }

  const listHeight = Math.min(Math.max(list.length, 1) * ROW_PX + 8, 260)
  const body =
    state === 'loading' ? (
      <p role="status" className="px-2 py-2 type-control text-[var(--color-text-muted)]">
        {MODEL_PICKER_WORDS.loading}
      </p>
    ) : state === 'error' ? (
      <div role="alert" className="flex items-center justify-between gap-2 px-2 py-2 type-control text-[var(--color-text-secondary)]">
        <span>{MODEL_PICKER_WORDS.error}</span>
        {onRetry ? (
          <button type="button" data-k2-focus="" className="k2-focus underline text-[var(--color-text-primary)]" onClick={onRetry}>
            {MODEL_PICKER_WORDS.retry}
          </button>
        ) : null}
      </div>
    ) : list.length === 0 ? (
      <p role="status" className="px-2 py-2 type-control text-[var(--color-text-muted)]" data-testid={`${testId}-no-match`}>
        {MODEL_PICKER_WORDS.noMatch(q.trim())}
      </p>
    ) : (
      <ScrollEdgeFrame className="w-full" scrollClassName="overflow-y-auto" tint="var(--color-bg)">
        <div role="listbox" id={`${base}-list`} aria-label={label} data-testid={`${testId}-list`}>
          {list.map((r, i) => {
            const on = r.value === checked
            return (
              <div
                key={r.value || '__default__'}
                id={optionId(i)}
                role="option"
                aria-selected={i === active}
                aria-checked={on}
                data-value={r.value}
                data-active={i === active ? '' : undefined}
                {...MENU_ROW}
                className={`flex cursor-pointer items-center gap-2 px-2 py-1.5 ${i === active ? 'bg-[var(--color-wash-1)]' : 'hover:bg-[var(--color-wash-1)]'}`}
                onMouseEnter={() => setActive(i)}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => pick(r)}
              >
                <span className="flex min-w-0 flex-1 flex-col">
                  <span className="type-control truncate text-[var(--color-text-primary)]">{r.label}</span>
                  {r.secondary ? <span className="type-hint truncate text-[var(--color-text-muted)]">{r.secondary}</span> : null}
                </span>
                {on ? (
                  <svg
                    data-testid={`${testId}-check`}
                    className="h-3.5 w-3.5 flex-shrink-0 text-[var(--color-accent-text)]"
                    fill="none"
                    viewBox="0 0 24 24"
                    stroke="currentColor"
                    strokeWidth={2.5}
                    aria-hidden="true"
                  >
                    <path strokeLinecap="round" strokeLinejoin="round" d="M5 13l4 4L19 7" />
                  </svg>
                ) : null}
              </div>
            )
          })}
        </div>
      </ScrollEdgeFrame>
    )

  return createPortal(
    <div data-k2-chamfer-float="menu" className="fixed z-[1000]" style={{ left: pos?.left ?? -9999, top: pos?.top ?? -9999 }}>
      <div
        ref={ref}
        role="dialog"
        aria-label={label}
        data-testid={testId}
        data-k2-chamfer="surface"
        {...floatMaterial('menu', 'frost')}
        {...MENU_LIST}
        className="flex flex-col gap-1.5 border border-[var(--color-border)] bg-[var(--color-bg)] [box-shadow:var(--shadow-3)]"
        style={{ width: MODEL_PICKER_WIDTH, maxHeight: 'calc(var(--k2-vh) * 0.6)' }}
        onKeyDown={(e) => {
          if (e.key === 'Escape') {
            e.preventDefault()
            e.stopPropagation()
            close(true)
          }
        }}
      >
        <SearchField
          ref={searchRef}
          value={q}
          escapeClears={false}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault()
              pick(list[active])
              return
            }
            const to = modelPickerMove(active, e.key, list.length)
            if (to !== null) {
              e.preventDefault()
              setActive(to)
            }
          }}
          placeholder={MODEL_PICKER_WORDS.search}
          aria-label={MODEL_PICKER_WORDS.search}
          role="combobox"
          aria-expanded={true}
          aria-controls={`${base}-list`}
          aria-activedescendant={state === 'ready' && list.length > 0 ? optionId(active) : undefined}
          data-testid={`${testId}-search`}
          spellCheck={false}
          autoComplete="off"
          className="w-full"
        />
        <div style={state === 'ready' && list.length > 0 ? { height: listHeight, display: 'flex' } : undefined}>{body}</div>
        <p className="px-1 type-hint text-[var(--color-text-muted)]" data-testid={`${testId}-footer`}>
          {footer}
        </p>
      </div>
    </div>,
    document.body,
  )
}
