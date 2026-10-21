// The Garden's own settings (0.45.6, Rosson 2026-10-15; ported from K2
// Companion's in-Zen settings sheet, `ZenSettings.tsx`). Reached from
// inside the Garden: the cog in Garden 1's Agents header (the Agents
// widget's `settings` prop). Never in the app's Settings window, which stays
// as it is.
//
// Groups (0.46.0):
//   - Appearance → Theme: the themes this computer's daemon offers
//     (`GET /cli/zen/theme/list?garden=`), the Garden's active one picked;
//     a pick is for THIS Garden only (`setZenTheme(name, 'garden', id)`).
//     The list failing (an older daemon, offline) leaves the row disabled
//     with a short note.
//   - Pinned agents (when the widget draws the pin grid): the grid's size,
//     read-only (the widget's `pin-limit`; Garden 1: 3 × 3). Pinned agents
//     past it head the list.
//   - Lists → "Recent agents first" (`zen-recent-first.ts`): on this
//     computer only. Until the person picks it shows the Garden's default
//     (the widget's `recent-first` prop; Garden 1 starts it on), else off.

import { useEffect, useState } from 'react'
import { SettingsIcon } from '@/components/icons/K2Icons'
import { useZenRecentFirst, useZenRecentFirstStore } from '@/lib/zen/zen-recent-first'
import { ZEN_LIGHT_BLUR_LABEL, ZEN_LIGHT_BLUR_NOTE, useZenLightBlurStore } from '@/lib/zen/zen-light-blur'
import { loadZenGardenThemes, setZenTheme } from '@/lib/zen/zen-theme-switch'
import type { ZenThemeEntry } from '@/lib/zen/zen-page'
import { zenPinGridText } from '@/lib/zen/zen-pin-grid'
import { ZenOverlay, zenButtonStyle } from './ZenOverlay'
import { ZEN_GLASS_PILL_PROPS, ZEN_TILE_PROPS } from '@/lib/zen/zen-materials'

export const ZEN_RECENT_FIRST_LABEL = 'Recent agents first'
export const ZEN_RECENT_FIRST_NOTE =
  'Below your pinned agents, the agents you last messaged from a Garden on this computer come first, on My Home and Agents. Garden 1 starts with it on; your choice here sticks on this computer only. The app’s Agents page doesn’t change.'
export const ZEN_GARDEN_THEME_NOTE = 'For this Garden only. Your other Gardens keep their theme.'
export const ZEN_GARDEN_THEME_FAILED_NOTE = 'Couldn’t load the themes. K2 on this computer may be older than this app.'
export const ZEN_PIN_GRID_NOTE = 'Set by this Garden. Pinned agents past the grid head the list below it.'
export const ZEN_PIN_GRID_NOTE_NO_LIMIT = 'Set by this Garden. The grid adds rows as you pin more.'

/** What the Agents widget hands its settings: which Garden, and the
 *  Garden's own defaults from the widget's props. */
export interface ZenGardenSettingsProps {
  /** The Garden on screen (`bridge.gardens.current()`), null when none. */
  gardenId: string | null
  /** The widget draws the pin grid (`pins`). */
  pinsOn: boolean
  /** The grid's cap (`pin-limit`), null for none. */
  pinLimit: number | null
  /** The Garden's "Recent agents first" default (`recent-first`), null for none. */
  recentFirstDefault: boolean | null
}

const rowStyle: React.CSSProperties = {
  gap: 12,
  padding: '8px 12px',
  borderRadius: 'var(--zen-radius)',
  background: 'var(--zen-surface)',
  border: '1px solid var(--zen-border)',
}
const noteStyle: React.CSSProperties = { fontSize: '0.8em', color: 'var(--zen-text-muted)' }

function Group({ id, title, children, data }: { id: string; title: string; children: React.ReactNode; data: string }): React.JSX.Element {
  return (
    <section aria-labelledby={id} {...{ [data]: '' }} className="flex flex-col" style={{ gap: 6 }}>
      <span
        id={id}
        style={{ fontSize: '0.75em', fontWeight: 600, letterSpacing: '0.04em', textTransform: 'uppercase', color: 'var(--zen-text-muted)' }}
      >
        {title}
      </span>
      {children}
    </section>
  )
}

function Switch({
  on,
  onToggle,
  labelledBy,
  marker = 'data-zen-recent-first-switch',
}: {
  on: boolean
  onToggle(): void
  labelledBy: string
  /** The switch's own data attribute (`on` / `off`), for tests and themes. */
  marker?: string
}): React.JSX.Element {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-labelledby={labelledBy}
      {...{ [marker]: on ? 'on' : 'off' }}
      onClick={onToggle}
      className="relative flex-shrink-0 cursor-pointer"
      style={{
        width: 36,
        height: 20,
        borderRadius: 999,
        border: 'none',
        background: on ? 'var(--zen-accent)' : 'var(--zen-border)',
        transition: 'background-color 150ms',
      }}
    >
      <span
        aria-hidden
        style={{
          position: 'absolute',
          top: 2,
          left: on ? 18 : 2,
          width: 16,
          height: 16,
          borderRadius: 999,
          background: on ? 'var(--zen-accent-text, #fff)' : 'var(--zen-surface-raised)',
          boxShadow: '0 1px 2px rgba(0, 0, 0, 0.25)',
          transition: 'left 150ms',
        }}
      />
    </button>
  )
}

type ThemeState =
  | { status: 'loading' }
  | { status: 'ready'; active: string | null; themes: ZenThemeEntry[] }
  | { status: 'failed' }

/** Appearance → Theme: this Garden's theme, picked for this Garden only. */
function ThemeRow({ gardenId }: { gardenId: string | null }): React.JSX.Element {
  const [state, setState] = useState<ThemeState>(gardenId ? { status: 'loading' } : { status: 'failed' })
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    if (!gardenId) {
      setState({ status: 'failed' })
      return
    }
    let live = true
    setState({ status: 'loading' })
    loadZenGardenThemes(gardenId).then(
      (t) => {
        if (live) setState({ status: 'ready', active: t.active, themes: t.themes })
      },
      (err: unknown) => {
        console.warn('[zen] Garden theme list failed:', err)
        if (live) setState({ status: 'failed' })
      },
    )
    return () => {
      live = false
    }
  }, [gardenId])

  const ready = state.status === 'ready' && state.themes.length > 0 ? state : null
  const pick = (name: string): void => {
    if (!ready || !gardenId || name === ready.active) return
    const label = ready.themes.find((t) => t.name === name)?.label ?? name
    setBusy(true)
    setError(null)
    setZenTheme(name, 'garden', gardenId).then(
      () => {
        setBusy(false)
        setState((s) => (s.status === 'ready' ? { ...s, active: name } : s))
      },
      (err: unknown) => {
        setBusy(false)
        setError(`Couldn’t switch to ${label}: ${err instanceof Error ? err.message : String(err)}`)
      },
    )
  }
  const failed = state.status === 'failed' || (state.status === 'ready' && state.themes.length === 0)
  return (
    <>
      <div className="flex items-center justify-between" style={rowStyle} {...ZEN_TILE_PROPS}>
        <label id="zen-garden-theme-label" htmlFor="zen-garden-theme-select">
          Theme
        </label>
        <select
          id="zen-garden-theme-select"
          data-zen-garden-theme-select={state.status}
          disabled={!ready || busy}
          value={ready?.active ?? ''}
          onChange={(e) => pick(e.target.value)}
          className="no-drag cursor-pointer"
          style={{
            minWidth: 140,
            padding: '4px 8px',
            borderRadius: 'calc(var(--zen-radius) - 4px)',
            border: '1px solid var(--zen-border)',
            background: 'var(--zen-surface-raised)',
            color: 'var(--zen-text)',
            font: 'inherit',
            opacity: ready ? 1 : 0.6,
          }}
        >
          {ready ? (
            <>
              {ready.active === null && <option value="">Choose…</option>}
              {ready.themes.map((t) => (
                <option key={t.name} value={t.name} data-zen-garden-theme-option={t.name}>
                  {t.label}
                </option>
              ))}
            </>
          ) : (
            <option value="">{state.status === 'loading' ? 'Loading…' : 'Unavailable'}</option>
          )}
        </select>
      </div>
      <span data-zen-garden-theme-note={failed ? 'failed' : 'ok'} style={noteStyle}>
        {failed ? ZEN_GARDEN_THEME_FAILED_NOTE : ZEN_GARDEN_THEME_NOTE}
      </span>
      {error && (
        <span role="alert" data-zen-garden-theme-error="" style={{ ...noteStyle, color: 'var(--zen-danger)' }}>
          {error}
        </span>
      )}
    </>
  )
}

export function ZenGardenSettings({
  onClose,
  gardenId,
  pinsOn,
  pinLimit,
  recentFirstDefault,
}: ZenGardenSettingsProps & { onClose(): void }): React.JSX.Element {
  const stored = useZenRecentFirstStore((s) => s.stored)
  const on = useZenRecentFirst(recentFirstDefault)
  const setOn = useZenRecentFirstStore((s) => s.setOn)
  const lightBlur = useZenLightBlurStore((s) => s.on)
  const setLightBlur = useZenLightBlurStore((s) => s.setOn)
  return (
    <ZenOverlay label="Garden settings" onClose={onClose} testId="zen-garden-settings" width={420}>
      <strong style={{ fontSize: '1.1em' }}>Garden settings</strong>
      <Group id="zen-garden-settings-appearance" title="Appearance" data="data-zen-garden-settings-appearance">
        <ThemeRow gardenId={gardenId} />
        <div className="flex items-center justify-between" style={rowStyle} {...ZEN_TILE_PROPS}>
          <span id="zen-light-blur-label">{ZEN_LIGHT_BLUR_LABEL}</span>
          <Switch
            on={lightBlur}
            onToggle={() => setLightBlur(!lightBlur)}
            labelledBy="zen-light-blur-label"
            marker="data-zen-light-blur-switch"
          />
        </div>
        <span data-zen-light-blur-note="" style={noteStyle}>
          {ZEN_LIGHT_BLUR_NOTE}
        </span>
      </Group>
      {pinsOn && (
        <Group id="zen-garden-settings-pins" title="Pinned agents" data="data-zen-garden-settings-pins">
          <div className="flex items-center justify-between" style={rowStyle} {...ZEN_TILE_PROPS}>
            <span>Grid</span>
            <span data-zen-pin-grid-text="" style={{ color: 'var(--zen-text-muted)' }}>
              {zenPinGridText(pinLimit)}
            </span>
          </div>
          <span data-zen-pin-grid-note="" style={noteStyle}>
            {pinLimit === null ? ZEN_PIN_GRID_NOTE_NO_LIMIT : ZEN_PIN_GRID_NOTE}
          </span>
        </Group>
      )}
      <Group id="zen-garden-settings-lists" title="Lists" data="data-zen-garden-settings-lists">
        <div className="flex items-center justify-between" style={rowStyle} {...ZEN_TILE_PROPS}>
          <span id="zen-recent-first-label">{ZEN_RECENT_FIRST_LABEL}</span>
          <Switch on={on} onToggle={() => setOn(!on)} labelledBy="zen-recent-first-label" />
        </div>
        <span
          data-zen-recent-first-note=""
          data-zen-recent-first-source={stored !== null ? 'stored' : recentFirstDefault !== null ? 'garden' : 'default'}
          style={noteStyle}
        >
          {ZEN_RECENT_FIRST_NOTE}
        </span>
      </Group>
      <div className="flex items-center justify-end">
        <button type="button" data-zen-garden-settings-done="" onClick={onClose} className="cursor-pointer" style={zenButtonStyle(true)}>
          Done
        </button>
      </div>
    </ZenOverlay>
  )
}

/** The cog that opens the Garden's settings (the Agents header). */
export function ZenGardenSettingsButton(props: ZenGardenSettingsProps): React.JSX.Element {
  const [open, setOpen] = useState(false)
  return (
    <>
      <button
        type="button"
        aria-label="Garden settings"
        title="Garden settings"
        aria-haspopup="dialog"
        data-zen-agents-settings=""
        data-zen-soft-button=""
        {...ZEN_GLASS_PILL_PROPS}
        onClick={() => setOpen(true)}
        className="no-drag flex flex-shrink-0 items-center justify-center cursor-pointer"
        style={{ width: 24, height: 24, borderRadius: 999, color: 'var(--zen-text-muted)' }}
      >
        {/* K2's one icon set, style A3 (Rosson 2026-10-16): Lucide `settings`, round caps. */}
        <SettingsIcon />
      </button>
      {open && <ZenGardenSettings {...props} onClose={() => setOpen(false)} />}
    </>
  )
}
