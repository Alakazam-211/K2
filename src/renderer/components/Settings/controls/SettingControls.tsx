// Settings type scale. Every Settings page uses these roles, and each role is
// one token and one weight (the utilities are in globals.css). Sizes change in
// styles/*/tokens.json, never at the call site. Nothing a person reads as a
// sentence goes below type-hint.
//
//   type-title    page title, stat value      --type-xl     500
//   type-heading  sentence-case subheading    --type-lg     600
//   type-section  uppercase section label     --type-xs     600
//   type-row      row label, row value, body  --type-md     400
//   type-name     name of a thing in a list   --type-md     500
//   type-hint     hint, description, status   --type-sm     400
//   type-control  button, input, dropdown     --type-md     400
//   type-badge    uppercase badge, count      --type-micro  600
//
// An unboxed text link takes type-hint, like the sentence around it.
// font-mono stays where a value or code is meant to be monospaced.

import React from 'react'
import { floatMaterial } from '@/components/ui/float-material'
import { createContext, useContext, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { Surface } from '@/components/ui/Surface'
import { Spinner } from '@/components/ui/Spinner'
import { useBrowserOverlay } from '@/lib/browser-overlay'
import { clientRectToCss, cssViewport } from '@/lib/app-zoom'

/** Set inside a card group, so a row paints the card's padding and leaves its
 *  divider to the card. */
const SettingsCardContext = createContext(false)

/** Width of a control column: text fields and dropdowns share it. Switches
 *  and buttons keep their own width on the same right edge. */
export const SETTING_FIELD_WIDTH = 'w-60'

/** A row's label column before its control wraps under it. A label and a
 *  control share a line while the label keeps this much room (10rem); past
 *  that the control drops under the label and hint, left-aligned at its own
 *  width, as a stacked row does (a narrow window, the app zoomed in). Flex
 *  wrapping, so it follows the row's real width at any app zoom, not a
 *  breakpoint. Pair with `ROW_WRAP` on the row. */
export const SETTING_ROW_LABEL_MIN = 'grow basis-40 min-w-0'
/** The row of a label beside a control that may wrap under it. */
export const ROW_WRAP = 'flex flex-wrap justify-between gap-x-6 gap-y-2'

/** Rounds an avatar in Glass (Terminal's `--radius-field` is 0, so the cut
 *  stays). Wrap the avatar, so other surfaces keep theirs. */
export const AVATAR_ROUND = 'inline-flex flex-shrink-0 overflow-hidden rounded-[var(--radius-field)]'

/** Row label and hint, one role each, so a label reads the same in every row.
 *  A hint wraps at a reading measure (about 75 characters a line), so a long
 *  one never runs the full width of a wide window. */
export const SETTING_ROW_LABEL = 'type-row text-[var(--color-text-primary)]'
export const SETTING_ROW_HINT = 'type-hint text-[var(--color-text-secondary)] mt-0.5 leading-relaxed max-w-[75ch]'
/** The server's own reason under a plain error line ("The server said: …"):
 *  the hint role, one step quieter than a hint (muted text still reads at
 *  4.5:1 on every ground), so the plain line leads. */
export const SETTING_SERVER_REASON = 'type-hint text-[var(--color-text-muted)] mt-1 leading-relaxed max-w-[75ch] break-words'

/** A list row that answers the pointer: the shared hover band
 *  (--color-row-hover, about 1.3:1 against the resting row on dark palettes
 *  and 1.2:1 on light ones), and on the same hover the row's muted text steps
 *  up to secondary, because muted text drops under 4.5:1 on the band. The
 *  trigger is the row's own :hover, which also holds while a control inside
 *  the row (Remove, a menu button) is under the pointer, so band and text
 *  always change together. Put it on the row, not on its children; a selected
 *  row swaps it for its own fill. */
export const SETTING_LIST_ROW_HOVER =
  'transition-colors hover:bg-[var(--color-row-hover)] hover:[--color-text-muted:var(--color-text-secondary)]'
/** The same band at rest, for a row the keyboard has highlighted. */
export const SETTING_LIST_ROW_BAND = 'bg-[var(--color-row-hover)] [--color-text-muted:var(--color-text-secondary)]'

export function SettingRow({
  label,
  hint,
  below,
  children,
  settingId,
  columns = false,
  testId,
  field = false,
  stack = false,
}: {
  label: React.ReactNode
  /** Under the label, in the hint role. */
  hint?: React.ReactNode
  /** Full-width content under the row: results, warnings, a second step. */
  below?: React.ReactNode
  children: React.ReactNode
  /**
   * Optional stable identifier used by the Settings search palette to
   * scroll this row into view and highlight it. Conventionally matches
   * a `SettingEntry.id` — e.g. `"terminal.font-family"`.
   */
  settingId?: string
  /** One fixed label column and one control column (the profile modal),
   *  instead of label left / control right. */
  columns?: boolean
  testId?: string
  /** A text field or dropdown: the control takes the shared field width. */
  field?: boolean
  /** The control goes under the label and hint, left-aligned, instead of on
   *  the right edge: for a control too wide to sit beside its hint (a set of
   *  choices in a narrow column). A field keeps the shared field width; any
   *  other control lays out across the row with notes beside it. */
  stack?: boolean
}): React.JSX.Element {
  const inCard = useContext(SettingsCardContext)
  if (stack) {
    return (
      <div className={inCard ? 'px-4 py-3' : 'py-2 border-b border-[var(--color-border)]'} data-settings-id={settingId} data-testid={testId}>
        <div className={SETTING_ROW_LABEL}>{label}</div>
        {hint && <div className={SETTING_ROW_HINT}>{hint}</div>}
        <div
          data-setting-row-control="stack"
          className={field ? `mt-2 ${SETTING_FIELD_WIDTH} max-w-full` : 'mt-2 flex flex-wrap items-center gap-2'}
        >
          {children}
        </div>
        {below && <div className="mt-2">{below}</div>}
      </div>
    )
  }
  if (columns) {
    return (
      <div
        className="flex items-center gap-3 py-2 border-b border-[var(--color-border)]"
        data-settings-id={settingId}
        data-testid={testId}
      >
        <span className="w-28 flex-shrink-0 type-row text-[var(--color-text-secondary)]">{label}</span>
        <div className="flex-1 min-w-0 flex items-center">{children}</div>
      </div>
    )
  }
  if (inCard || hint !== undefined || below !== undefined) {
    return (
      <div className={inCard ? 'px-4 py-3' : 'py-2 border-b border-[var(--color-border)]'} data-settings-id={settingId} data-testid={testId}>
        <div className={`${ROW_WRAP} ${hint ? 'items-start' : 'items-center'}`}>
          <div className={SETTING_ROW_LABEL_MIN}>
            <div className={SETTING_ROW_LABEL}>{label}</div>
            {hint && <div className={SETTING_ROW_HINT}>{hint}</div>}
          </div>
          <div
            data-setting-row-control="side"
            className={field ? `${SETTING_FIELD_WIDTH} max-w-full flex-shrink-0` : 'flex flex-wrap items-center gap-2 flex-shrink-0 max-w-full'}
          >
            {children}
          </div>
        </div>
        {below && <div className="mt-2">{below}</div>}
      </div>
    )
  }
  return (
    <div
      className="flex items-center justify-between py-2 border-b border-[var(--color-border)]"
      data-settings-id={settingId}
      data-testid={testId}
    >
      <span className="type-row text-[var(--color-text-secondary)]">{label}</span>
      {children}
    </div>
  )
}

/** What the last write from a row did: nothing yet, saved (for a moment), or
 *  failed with a reason. */
export type SaveState = 'idle' | 'saved' | { error: string }

/** A row's save confirmation. Call `saved()` when a write lands and
 *  `failed(err)` when it doesn't; "Saved" shows for `holdMs`, an error stays
 *  until the next try. Pair with <SaveNote> by the control and <SaveError>
 *  under the hint. */
export function useSaveStatus(holdMs = 2000): {
  state: SaveState
  saved: () => void
  failed: (err: unknown) => void
  reset: () => void
} {
  const [state, setState] = useState<SaveState>('idle')
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const clear = (): void => {
    if (timer.current) clearTimeout(timer.current)
    timer.current = null
  }
  useEffect(() => clear, [])
  return {
    state,
    saved: () => {
      clear()
      setState('saved')
      timer.current = setTimeout(() => setState('idle'), holdMs)
    },
    failed: (err) => {
      clear()
      const raw = err instanceof Error ? err.message : typeof err === 'string' ? err : ''
      setState({ error: raw.trim() || 'K2 couldn\u2019t reach this server.' })
    },
    reset: () => {
      clear()
      setState('idle')
    },
  }
}

/** "Saved" with a check, quiet, beside the control for a moment after a write. */
export function SaveNote({ state }: { state: SaveState }): React.JSX.Element | null {
  if (state !== 'saved') return null
  return (
    <span role="status" data-save-note="" className="inline-flex items-center gap-1 type-hint text-[var(--color-status-ok-soft)] whitespace-nowrap">
      <svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" className="w-3 h-3">
        <path d="M3.5 8.5 6.5 11.5 12.5 4.5" />
      </svg>
      Saved
    </span>
  )
}

/** Why the last write failed, under the row's hint. The control is already
 *  back where it was; this says so and that trying again is safe. */
export function SaveError({ state }: { state: SaveState }): React.JSX.Element | null {
  if (typeof state !== 'object') return null
  return (
    <p role="alert" data-save-error="" className="type-hint text-[var(--color-status-error-soft)] mt-1 leading-relaxed max-w-[75ch]">
      Couldn&rsquo;t save: {state.error.replace(/[.\s]+$/, '')}. Nothing changed; try again.
    </p>
  )
}

/** What a page or card says while its data is loading, after a read failed,
 *  or when there is nothing yet. One card, the same on every page: a short
 *  title saying what happened, a plain line under it, and the next step on
 *  the right (Try again, Add …). Use it in place of the card the data would
 *  fill, so the page keeps its shape. `detail` is the server's own reason,
 *  shown quietly after the plain line, never instead of it. */
export function SettingsLoadState({
  kind,
  title,
  children,
  detail,
  action,
  testId,
}: {
  kind: 'loading' | 'error' | 'empty'
  title: string
  children?: React.ReactNode
  detail?: string | null
  action?: React.ReactNode
  testId?: string
}): React.JSX.Element {
  const reason = detail?.trim().replace(/[.\s]+$/, '')
  return (
    <Surface
      chamfer
      material="tile"
      role={kind === 'error' ? 'alert' : 'status'}
      aria-busy={kind === 'loading' || undefined}
      data-load-state={kind}
      data-testid={testId}
      className="px-4 py-3"
    >
      <div className="flex items-center justify-between gap-6">
        <div className="flex items-start gap-2 min-w-0">
          {kind === 'loading' && <Spinner className="mt-0.5" />}
          <div className="min-w-0">
            <p className={kind === 'error' ? 'type-row text-[var(--color-status-error-soft)]' : SETTING_ROW_LABEL}>{title}</p>
            {children && <p className={SETTING_ROW_HINT}>{children}</p>}
            {reason && <p className={SETTING_SERVER_REASON} data-load-detail="">The server said: {reason}.</p>}
          </div>
        </div>
        {action && <div className="flex items-center gap-2 flex-shrink-0">{action}</div>}
      </div>
    </Surface>
  )
}

/** A row whose left side is free-form (label, hint, notes) and whose control
 *  is a switch or small cluster on the right. `compact` is the narrow room
 *  drawer: the control leads and the row keeps its tighter padding. */
export function SettingSwitchRow({
  control,
  children,
  compact = false,
  testId,
  settingId,
}: {
  control: React.ReactNode
  children: React.ReactNode
  compact?: boolean
  testId?: string
  settingId?: string
}): React.JSX.Element {
  if (compact) {
    return (
      <div className="px-3 py-1.5" data-testid={testId} data-settings-id={settingId}>
        <div className="flex items-start gap-3">
          {control}
          <div className="flex-1 min-w-0">{children}</div>
        </div>
      </div>
    )
  }
  return (
    <div className="px-4 py-3" data-testid={testId} data-settings-id={settingId}>
      <div className="flex items-start justify-between gap-6">
        <div className="flex-1 min-w-0">{children}</div>
        <div className="flex items-center gap-2 flex-shrink-0">{control}</div>
      </div>
    </div>
  )
}

export function SettingsGroup({
  title,
  badge,
  hint,
  action,
  card = false,
  state,
  children,
  settingId,
  testId,
}: {
  /** Card groups may leave it out: a card that continues the group above
   *  it (no heading, no hint, no action) starts straight with its rows. */
  title?: string
  /** Optional inline badge rendered next to the title — used for
   *  `beta` / status tags. Stays opt-in so existing callers
   *  (Workspace, Worktrees, Chat Migrations) render unchanged. */
  badge?: React.ReactNode
  /** One line under the title (card groups). */
  hint?: React.ReactNode
  /** The group's one action, on the title's right (card groups). */
  action?: React.ReactNode
  /** Rows in one card: chamfered in Terminal, a tile in Glass, hairlines
   *  between rows. The default keeps the section label and left rail. */
  card?: boolean
  /** Card groups: shown in place of the card, under the same title, while
   *  the group's data loads, after its read failed, or when it is empty
   *  (a <SettingsLoadState>). The children are not rendered. */
  state?: React.ReactNode
  children?: React.ReactNode
  settingId?: string
  testId?: string
}): React.JSX.Element {
  if (card) {
    return (
      <section data-settings-id={settingId} data-testid={testId}>
        {(title !== undefined || hint || action) && (
        <div className="mb-2">
          {/* The action drops under the title when they don't fit one line. */}
          <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 min-h-[1.5rem]">
            <h3 className="type-heading text-[var(--color-text-primary)] flex items-center gap-2">
              <span>{title}</span>
              {badge}
            </h3>
            {action}
          </div>
          {hint && <p className={SETTING_ROW_HINT}>{hint}</p>}
        </div>
        )}
        {state ?? (
          <Surface chamfer material="tile" className="divide-y divide-[var(--color-border)]">
            <SettingsCardContext.Provider value={true}>{children}</SettingsCardContext.Provider>
          </Surface>
        )}
      </section>
    )
  }
  return (
    <div className="space-y-2">
      <h3 className="type-section text-[var(--color-text-muted)] uppercase tracking-wider flex items-center gap-2">
        <span>{title}</span>
        {badge}
      </h3>
      <div className="ml-2 pl-3 border-l-2 border-[var(--color-border)] space-y-1">
        {children}
      </div>
    </div>
  )
}

/** Overflow-clipping ancestor (or the viewport) — used to flip menus up
 *  when a trigger sits at the bottom of a panel with overflow:hidden. */
function clipRectFor(el: HTMLElement): DOMRect {
  let n: HTMLElement | null = el.parentElement
  while (n) {
    const s = getComputedStyle(n)
    if (/(auto|hidden|scroll|clip)/.test(s.overflowY) || /(auto|hidden|scroll|clip)/.test(s.overflowX)) {
      return n.getBoundingClientRect()
    }
    n = n.parentElement
  }
  return new DOMRect(0, 0, window.innerWidth, window.innerHeight)
}

function shouldOpenMenuUp(trigger: HTMLElement, optionCount: number): boolean {
  // The menu's size is CSS px; the rects and the window are window px.
  const t = clientRectToCss(trigger.getBoundingClientRect())
  const clip = clientRectToCss(clipRectFor(trigger))
  const gap = 4
  const menuMax = 240
  const estimated = Math.min(menuMax, Math.max(optionCount, 1) * 28)
  const spaceBelow = Math.min(cssViewport().height, clip.bottom) - t.bottom - gap
  const spaceAbove = t.top - Math.max(0, clip.top) - gap
  return spaceBelow < estimated && spaceAbove > spaceBelow
}

const MENU_LAYER_FLOOR = 400

/** Layer for the portaled menu. Floor 400 stays under the assistant bar.
 *  One above the highest ancestor integer z-index. `auto` and any
 *  non-integer are skipped. Position is not a filter: a static flex or
 *  grid item with a numeric z-index is a layer, and jsdom reports the
 *  dialog frame as static while keeping its inline z-index. */
export function menuLayerForTrigger(trigger: HTMLElement): number {
  let highest: number | null = null
  for (let el: Element | null = trigger.parentElement; el; el = el.parentElement) {
    const raw = getComputedStyle(el).zIndex.trim()
    if (!/^-?\d+$/.test(raw)) continue
    const value = Number(raw)
    if (highest === null || value > highest) highest = value
  }
  if (highest === null) return MENU_LAYER_FLOOR
  return Math.max(MENU_LAYER_FLOOR, highest + 1)
}

export function SettingDropdown({
  value,
  options,
  onChange,
  className,
  placeholder,
  menuAlign = 'right',
  menuPlacement = 'auto',
  fullWidth = false,
  disabled = false,
  ariaLabel,
  triggerLabel,
  title,
  compact = false,
  triggerData,
}: {
  value: string
  /** `group` starts a heading row when it differs from the previous
   *  option's; `badge` is short trailing text (e.g. "In use"). */
  options: {
    value: string
    label: string
    disabled?: boolean
    leading?: React.ReactNode
    group?: string
    badge?: string
  }[]
  onChange: (value: string) => void | Promise<void>
  className?: string
  /**
   * Which edge the open menu is anchored to. Default `'right'` (the menu
   * grows leftward as it widens) matches every existing call site. Pass
   * `'left'` for a trigger sitting at the LEFT edge of a container, so a
   * wide menu opens rightward into the open space instead of extending
   * left past the container and getting clipped.
   */
  menuAlign?: 'left' | 'right'
  /**
   * Vertical placement. `'auto'` (default) opens down unless the trigger
   * sits too close to the bottom of its clipping ancestor (Create database
   * workspace picker, etc.), then opens up so the list stays visible.
   */
  menuPlacement?: 'up' | 'down' | 'auto'
  /**
   * When provided, a `value` that matches no option shows this placeholder
   * (muted) INSTEAD of silently falling back to `options[0]`. Without it the
   * dropdown would display the first option as if it were chosen, even though
   * the caller never committed a selection — a phantom default that reads as
   * "selected" but isn't. Pass a placeholder anywhere "nothing picked yet" is
   * a real, distinct state the user must resolve.
   */
  placeholder?: string
  /** Stretch the trigger across the container (federated server picker, etc.). */
  fullWidth?: boolean
  /** Block opening and dim the trigger. Omitted callers stay enabled. */
  disabled?: boolean
  /** Accessible name for the trigger. Omitted callers are unchanged. */
  ariaLabel?: string
  /** Fixed text on the closed trigger instead of the selected option's
   *  label (the open menu still marks the selected one). */
  triggerLabel?: string
  /** Hover text on the trigger. */
  title?: string
  /** A toolbar-height trigger (h-6) that truncates inside the container's
   *  max width (the Mobile toolbar). Omitted callers are unchanged. */
  compact?: boolean
  /** `data-*` attributes for the trigger button (tests, automation). */
  triggerData?: Record<`data-${string}`, string>
}): React.JSX.Element {
  const [isOpen, setIsOpen] = useState(false)
  const [menuBox, setMenuBox] = useState<{
    top?: number
    bottom?: number
    left?: number
    right?: number
    minWidth: number
    zIndex: number
    /** Off when the trigger lives in a Garden or the Thread box: the menu is
     *  portaled out of their scope, so the chamfer rule can't see it. */
    chamfer: boolean
  } | null>(null)
  const containerRef = useRef<HTMLDivElement>(null)
  const menuRef = useRef<HTMLDivElement>(null)
  // Used outside Settings too (Presets bar, dialogs): claim the open list
  // over a native Browser page.
  useBrowserOverlay({ active: isOpen && menuBox !== null, ref: menuRef, kind: 'menu', watch: menuBox })

  const match = options.find((o) => o.value === value)
  const selected = match ?? (placeholder !== undefined ? undefined : options[0])

  useEffect(() => {
    if (!disabled) return
    setIsOpen(false)
  }, [disabled])

  useLayoutEffect(() => {
    if (!isOpen) {
      setMenuBox(null)
      return
    }
    const place = (): void => {
      const trigger = containerRef.current
      if (!trigger) return
      // The rect and the window are window px; the menu's fixed edges are
      // CSS px, which the app zoom scales (lib/app-zoom).
      const rect = clientRectToCss(trigger.getBoundingClientRect())
      const view = cssViewport()
      const up =
        menuPlacement === 'up'
          ? true
          : menuPlacement === 'down'
            ? false
            : shouldOpenMenuUp(trigger, options.length)
      setMenuBox({
        minWidth: rect.width,
        zIndex: menuLayerForTrigger(trigger),
        chamfer: trigger.closest('[data-zen-root], [data-k2-thread-zen]') === null,
        ...(up
          ? { bottom: view.height - rect.top + 2 }
          : { top: rect.bottom + 2 }),
        ...(menuAlign === 'left'
          ? { left: rect.left }
          : { right: view.width - rect.right }),
      })
    }
    place()
    window.addEventListener('resize', place)
    window.addEventListener('scroll', place, true)
    return () => {
      window.removeEventListener('resize', place)
      window.removeEventListener('scroll', place, true)
    }
  }, [isOpen, menuAlign, menuPlacement, options.length])

  useEffect(() => {
    if (!isOpen) return
    const handler = (e: MouseEvent): void => {
      const node = e.target as Node
      if (containerRef.current?.contains(node)) return
      if (menuRef.current?.contains(node)) return
      setIsOpen(false)
    }
    document.addEventListener('mousedown', handler)
    return () => document.removeEventListener('mousedown', handler)
  }, [isOpen])

  // Esc closes the open menu only (0.46.3), never the Settings panel too:
  // the capture-phase listener runs before Settings' window listener, and
  // `preventDefault` makes that one stand down (`escOwnedByDialog`).
  useEffect(() => {
    if (!isOpen) return
    const onKey = (e: KeyboardEvent): void => {
      if (e.key !== 'Escape' || e.defaultPrevented) return
      e.preventDefault()
      e.stopPropagation()
      setIsOpen(false)
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [isOpen])

  const toggleOpen = (): void => {
    if (disabled) return
    setIsOpen(!isOpen)
  }

  return (
    <div ref={containerRef} className={`relative no-drag ${fullWidth ? 'w-full' : ''} ${className ?? ''}`}>
      <button
        type="button"
        onClick={toggleOpen}
        disabled={disabled}
        aria-label={ariaLabel}
        title={title}
        data-k2-chamfer="control"
        {...triggerData}
        className={`k2-focus flex items-center type-control ${compact ? 'h-6 max-w-full gap-1 px-1.5' : 'gap-2 px-2 py-1'} bg-[var(--color-bg)] border border-[var(--color-control-edge)] text-[var(--color-text-primary)] transition-colors ${
          disabled
            ? 'opacity-50 cursor-not-allowed'
            : 'hover:border-[var(--color-text-muted)] cursor-pointer'
        } ${fullWidth ? 'w-full justify-between' : ''}`}
      >
        <span
          className={`flex min-w-0 items-center gap-2 truncate ${selected || triggerLabel !== undefined ? '' : 'text-[var(--color-text-muted)]'}`}
        >
          {triggerLabel === undefined && selected?.leading}
          <span className="truncate">{triggerLabel ?? selected?.label ?? placeholder ?? ''}</span>
        </span>
        <svg
          className={`w-3 h-3 text-[var(--color-text-muted)] flex-shrink-0 transition-transform ${isOpen ? 'rotate-180' : ''}`}
          fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}
        >
          <path strokeLinecap="round" strokeLinejoin="round" d="M19 9l-7 7-7-7" />
        </svg>
      </button>

      {isOpen && menuBox && createPortal(
        <div
          ref={menuRef}
          style={{
            position: 'fixed',
            zIndex: menuBox.zIndex,
            minWidth: menuBox.minWidth,
            top: menuBox.top,
            bottom: menuBox.bottom,
            left: menuBox.left,
            right: menuBox.right,
          }}
          data-testid="setting-dropdown-menu"
          data-setting-dropdown-menu=""
          data-k2-chamfer-float={menuBox.chamfer ? 'menu' : undefined}
          className="w-max"
        >
          <div
            data-k2-chamfer={menuBox.chamfer ? 'surface' : undefined}
            {...floatMaterial('menu', menuBox.chamfer ? 'frost' : undefined)} className="bg-[var(--color-bg)] border border-[var(--color-border)] shadow-xl"
            style={{ '--k2-menu-origin': `${menuBox.bottom !== undefined ? 'bottom' : 'top'} ${menuBox.right !== undefined ? 'right' : 'left'}` } as React.CSSProperties}
          >
          <div className="max-h-60 overflow-y-auto">
          {options.map((option, i) => {
            const isActive = option.value === value
            const isDisabled = option.disabled === true
            const heading =
              option.group !== undefined && option.group !== options[i - 1]?.group ? option.group : null
            return (
              <React.Fragment key={option.value}>
              {heading !== null && (
                <div
                  role="presentation"
                  className="px-3 pt-2 pb-0.5 type-section uppercase tracking-wider text-[var(--color-text-muted)]"
                >
                  {heading}
                </div>
              )}
              <button
                type="button"
                disabled={isDisabled}
                data-value={option.value}
                data-selected={isActive ? '' : undefined}
                onClick={() => {
                  if (disabled || isDisabled) return
                  onChange(option.value)
                  setIsOpen(false)
                }}
                className={`w-full flex items-center gap-2 px-3 py-1.5 text-left type-control transition-colors focus:outline-none focus-visible:outline-2 focus-visible:outline-solid focus-visible:-outline-offset-2 focus-visible:outline-[var(--color-accent-text)] ${
                  isDisabled
                    ? 'text-[var(--color-text-muted)] opacity-50 cursor-not-allowed'
                    : isActive
                      ? 'text-[var(--color-accent)] bg-[var(--color-accent)]/10 cursor-pointer'
                      : 'text-[var(--color-text-secondary)] hover:bg-[var(--color-wash-1)] hover:text-[var(--color-text-primary)] cursor-pointer'
                }`}
              >
                {option.leading}
                <span className="whitespace-nowrap flex-1">{option.label}</span>
                {option.badge && (
                  <span className="type-badge uppercase tracking-wider text-[var(--color-text-muted)] flex-shrink-0">
                    {option.badge}
                  </span>
                )}
                {isActive && (
                  <svg className="w-3 h-3 flex-shrink-0 text-[var(--color-accent)]" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
                    <path strokeLinecap="round" strokeLinejoin="round" d="M5 13l4 4L19 7" />
                  </svg>
                )}
              </button>
              </React.Fragment>
            )
          })}
          </div>
          </div>
        </div>,
        document.body,
      )}
    </div>
  )
}
