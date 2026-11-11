// Settings → Styles (Style System P3, prd-style-system-v1 §5 / ST5).
//
// List and detail, like Workspaces / Agents side by side: the styles on the
// left (the one in use tagged), the shown style's settings on the right in
// card groups: Colours, Layout or Effects, Terminals, and a live Preview.
//
// Interaction contract:
// - HOVER previews are strictly attribute-level (stampStyleAttributes)
//   with no store/state/persistence writes, so a stray hover can never
//   race a commit; mouse-leave restores the committed selection's
//   attributes.
// - CLICK commits: applyStyle (store + <html> + localStorage mirror)
//   then updateStyleSettings (settings-store mirror only — style is
//   per-client view state, never daemon-canonical; multi-window sync
//   is via localStorage `storage` events).
// - Every save is read back from localStorage. A change that applied but
//   didn't save says so in its row, instead of failing silently.

import React, { useCallback, useEffect, useReducer, useRef, useState } from 'react'
import { useSettingsStore } from '@/stores/settings'
import { LS_PALETTE, LS_SCHEME, LS_STYLE, osPrefersLight, stampStyleAttributes, useStyleStore } from '@/stores/style'
import { useTerminalSettingsStore } from '@/stores/terminal-settings'
import { DEFAULT_SELECTION, type StyleSelection } from '@/lib/style-resolve'
import { dialDefault, dialStorageKey, formatDialValue, resolveDialValue } from '@/lib/style-dials'
import { accentFillHex, accentStorageKey } from '@/lib/style-accent'
import { readStoredTextGamma, TEXT_GAMMA_MAX, TEXT_GAMMA_MIN, textGammaStorageKey } from '@/lib/text-gamma'
import {
  STYLES,
  type StyleDialMeta,
  type StyleMeta,
  type StylePaletteMeta,
  type StyleScheme,
} from '@/styles.generated'
import type { SettingEntry } from '../searchManifest'
import { SaveNote, SettingRow, SettingsGroup } from '../controls/SettingControls'
import { Button, Input, Segmented, Slider, Tag, cx } from '@/components/ui'

/** Registry terminal colors are 0xRRGGBB ints (Kessel's format). */
const intToHex = (n: number): string => `#${n.toString(16).padStart(6, '0')}`

export const STYLES_MANIFEST: SettingEntry[] = [
  { id: 'styles.style-picker', section: 'styles', label: 'Style', description: 'How the whole app looks: Terminal or Glass', keywords: ['style', 'skin', 'theme', 'appearance', 'look', 'terminal', 'square', 'glass', 'round', 'calm'] },
  { id: 'styles.palette', section: 'styles', label: 'Palette', description: 'The set of colours the style uses', keywords: ['palette', 'color', 'colors', 'colour', 'colours', 'theme', 'charcoal', 'paper', 'obsidian', 'veil', 'dark mode', 'light mode'] },
  { id: 'styles.scheme', section: 'styles', label: 'Light or dark', description: 'Light, Dark, or Auto to follow your system', keywords: ['scheme', 'light', 'dark', 'auto', 'appearance', 'mode', 'night', 'day', 'os', 'system'] },
  { id: 'styles.auto-palettes', section: 'styles', label: 'Palettes for Auto', description: 'Under Auto, the palette for a light system and for a dark system', keywords: ['auto', 'system', 'light', 'dark', 'swap', 'palette', 'theme', 'yellow'] },
  { id: 'styles.accent', section: 'styles', label: 'Accent colour', description: 'The colour of buttons, switches, links and focus rings, with a reset', keywords: ['accent', 'colour', 'color', 'tint', 'highlight', 'custom', 'reset', 'picker', 'hex'] },
  { id: 'styles.density', section: 'styles', label: 'Spacing', description: 'Compact, Regular or Spacious room between panes and tiles', keywords: ['density', 'gaps', 'spacing', 'compact', 'regular', 'spacious', 'padding', 'tiles'] },
  { id: 'styles.dials', section: 'styles', label: 'Frost', description: 'How much Glass blurs what is behind menus and popovers', keywords: ['dials', 'frost', 'blur', 'adjust', 'tweak', 'slider', 'glass', 'effects', 'translucency'] },
  { id: 'styles.text-weight', section: 'styles', label: 'Terminal text weight', description: 'How thick text looks in terminals, saved for each style', keywords: ['gamma', 'weight', 'thickness', 'bold', 'thin', 'chonky', 'webgl', 'text', 'terminal'] },
]

/** Retired looks stay compiled but never reach the picker. */
// The default look first: Glass, then Terminal (Rosson, 2026-11-11).
const PICKER_STYLES = STYLES.filter((s) => !s.hidden).sort(
  (a, b) => Number(b.id === DEFAULT_SELECTION.styleId) - Number(a.id === DEFAULT_SELECTION.styleId),
)

const AUTO_SIDES: { side: StyleScheme; label: string }[] = [
  { side: 'light', label: 'When my system is light' },
  { side: 'dark', label: 'When my system is dark' },
]

const SCHEME_OPTIONS: { id: 'light' | 'dark' | 'auto'; label: string }[] = [
  { id: 'light', label: 'Light' },
  { id: 'dark', label: 'Dark' },
  { id: 'auto', label: 'Auto' },
]

/** What a dial changes, in plain words, by `<style>.<dial>`. */
const DIAL_HINTS: Record<string, string> = {
  'glass.frost': 'How much menus and popovers blur what is behind them. 0 is clear glass.',
}

const NOT_SAVED =
  "This shows now, but K2 couldn't save it on this computer, so it won't last. Try the change again."

/** Small shapes inside a card: square on Terminal; on Glass a fraction of the
 *  card's radius, so they sit concentric within its padding. */
const SWATCH_SHAPE = 'rounded-[calc(var(--radius-box)/4)]'


/** A slider's value, in a fixed slot after it, so every slider in the page
 *  ends on the same edge whether or not it shows a value. */
const VALUE_SLOT = 'w-10 flex-shrink-0 text-right type-hint text-[var(--color-text-secondary)] tabular-nums'

const HEX_RE = /^#?([0-9a-f]{6})$/i

/** A localStorage value, or undefined when storage can't be read. */
function readKey(key: string): string | null | undefined {
  try {
    return localStorage.getItem(key)
  } catch {
    return undefined
  }
}

/** Colour chips side by side in one outlined strip. The outline is the
 *  control edge, so a dark chip reads on a dark card and a light one on light. */
function SwatchStrip({ colors, className }: { colors: readonly string[]; className: string }): React.JSX.Element {
  return (
    <span aria-hidden="true" className={cx('flex overflow-hidden border border-[var(--color-control-edge)]', SWATCH_SHAPE, className)}>
      {colors.map((c, i) => (
        <span key={i} className="min-w-0 flex-1" style={{ backgroundColor: c }} />
      ))}
    </span>
  )
}

/** The chosen palette's tick. In the text colour: the card's edge is its one
 *  accent cue. */
function CheckMark(): React.JSX.Element {
  return (
    <svg className="w-3 h-3 flex-shrink-0 text-[var(--color-text-primary)]" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5} aria-hidden="true">
      <path strokeLinecap="round" strokeLinejoin="round" d="M5 13l4 4L19 7" />
    </svg>
  )
}

/** A row's error line, under its controls. */
function RowError({ id, children }: { id?: string; children: React.ReactNode }): React.JSX.Element {
  return (
    <p id={id} role="alert" className="type-hint text-[var(--color-status-error-text)] leading-relaxed">
      {children}
    </p>
  )
}

function capitalize(s: string): string {
  return s.length ? s[0].toUpperCase() + s.slice(1) : s
}

/** Which appearance a palette is for, in a word or two. */
function schemeWords(p: StylePaletteMeta): string {
  if (p.schemes.includes('dark') && p.schemes.includes('light')) return 'Dark or light'
  return p.schemes.includes('light') ? 'Light' : 'Dark'
}

/** Density options: '' is every style's base (flush/compact) density,
 *  then whatever gap presets the style declares, in declared order. */
function densityOptions(style: StyleMeta): { id: string; label: string }[] {
  return [{ id: '', label: 'Compact' }, ...style.gapPresets.map((p) => ({ id: p, label: capitalize(p) }))]
}

/** The default selection when jumping INTO a style (style row click /
 *  cross-style palette click): keep the user's scheme mode + density
 *  where the style can honor them, fall back where it can't. */
function selectionForStyle(style: StyleMeta, current: StyleSelection): StyleSelection {
  const paletteId =
    current.styleId === style.id
      ? current.paletteId
      : (style.defaultPalettes[schemeModeHint(current)] ?? style.defaultPalette)
  return {
    styleId: style.id,
    paletteId,
    schemeMode: current.schemeMode,
    gapsPreset: style.capabilities.gaps && style.gapPresets.includes(current.gapsPreset) ? current.gapsPreset : '',
  }
}

/** Best static guess of the scheme a mode resolves to (for picking a
 *  default palette without re-reading matchMedia — resolution proper
 *  happens in the style store on apply). */
function schemeModeHint(sel: StyleSelection): StyleScheme {
  return sel.schemeMode === 'light' ? 'light' : 'dark'
}

/** Where a change was made, so a save that failed is reported in its row. */
type SaveSpot = 'style' | 'palette' | 'scheme' | 'auto' | 'accent' | 'density' | 'dials' | 'text-weight'

export function StylesSection(): React.JSX.Element {
  const styleId = useStyleStore((s) => s.styleId)
  const paletteId = useStyleStore((s) => s.paletteId)
  const schemeMode = useStyleStore((s) => s.schemeMode)
  const gapsPreset = useStyleStore((s) => s.gapsPreset)
  const autoPalettes = useStyleStore((s) => s.autoPalettes)
  const resolvedScheme = useStyleStore((s) => s.resolvedScheme)
  const resolvedPaletteId = useStyleStore((s) => s.resolvedPaletteId)
  const applyStyle = useStyleStore((s) => s.applyStyle)
  const textGamma = useStyleStore((s) => s.textGamma)
  const setTextGamma = useStyleStore((s) => s.setTextGamma)
  const resetTextGamma = useStyleStore((s) => s.resetTextGamma)
  const accent = useStyleStore((s) => s.accent)
  const setAccent = useStyleStore((s) => s.setAccent)
  const resetAccent = useStyleStore((s) => s.resetAccent)
  const updateStyleSettings = useSettingsStore((s) => s.updateStyleSettings)
  const openSettings = useSettingsStore((s) => s.openSettings)
  const painter = useTerminalSettingsStore((s) => s.painter)

  const committed: StyleSelection = { styleId, paletteId, schemeMode, gapsPreset, autoPalettes }

  // The accent belongs to the style in use, whichever style's detail is showing.
  const activeStyle = STYLES.find((s) => s.id === styleId) ?? PICKER_STYLES[0]
  const activePalette = activeStyle.palettes.find((p) => p.id === resolvedPaletteId) ?? activeStyle.palettes[0]
  const accentHex = accentFillHex(accent, activePalette, resolvedScheme)

  // Which style's DETAIL is showing. Defaults to the active style; kept
  // as local state so browsing a style's palettes doesn't commit it.
  const [selectedStyleId, setSelectedStyleId] = useState(styleId)
  const selectedStyle = PICKER_STYLES.find((s) => s.id === selectedStyleId) ?? PICKER_STYLES[0]
  const selectedIsActive = selectedStyle.id === styleId

  // A save that applied live but didn't reach storage, by where it was made.
  const [unsaved, setUnsaved] = useState<SaveSpot | null>(null)
  // A save that did land, by where it was made: "Saved" there for 2s.
  const [justSaved, setJustSaved] = useState<SaveSpot | null>(null)
  const savedTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  useEffect(() => () => {
    if (savedTimer.current) clearTimeout(savedTimer.current)
  }, [])
  const noteSave = (spot: SaveSpot, saved: boolean): void => {
    setUnsaved((cur) => (saved ? (cur === spot ? null : cur) : spot))
    if (savedTimer.current) clearTimeout(savedTimer.current)
    setJustSaved(saved ? spot : null)
    if (saved) savedTimer.current = setTimeout(() => setJustSaved(null), 2000)
  }
  const savedNote = (spot: SaveSpot): React.JSX.Element => <SaveNote state={justSaved === spot ? 'saved' : 'idle'} />
  // Storage-derived state (is there anything to reset?) re-reads on render.
  const [, rerender] = useReducer((n: number) => n + 1, 0)

  const commit = useCallback(
    (sel: StyleSelection, spot: SaveSpot): void => {
      applyStyle(sel) // instant: store + <html> + mirror
      void updateStyleSettings({
        id: sel.styleId,
        palette: sel.paletteId,
        scheme: sel.schemeMode,
        gaps: sel.gapsPreset,
      })
      const now = useStyleStore.getState()
      noteSave(
        spot,
        readKey(LS_STYLE) === now.styleId &&
          readKey(LS_SCHEME) === now.schemeMode &&
          readKey(LS_PALETTE) === now.resolvedPaletteId,
      )
    },
    [applyStyle, updateStyleSettings],
  )

  // ── Hover preview (attribute-level only; see header comment) ──────
  const preview = useCallback((sel: StyleSelection): void => {
    stampStyleAttributes(sel)
  }, [])
  const endPreview = useCallback((): void => {
    stampStyleAttributes(committed)
    // committed is rebuilt each render from store state — safe to close over.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [styleId, paletteId, schemeMode, gapsPreset, autoPalettes])

  // Base selection edits in the detail pane operate on: the committed
  // selection when the active style is showing, else a jump-in default.
  const baseSel = selectedIsActive ? committed : selectionForStyle(selectedStyle, committed)

  /** The OS appearance right now: which Auto pick is on screen. */
  const osSide = (): StyleScheme => (osPrefersLight() ? 'light' : 'dark')

  /** The selection with one Auto side pointed at a palette. */
  const withAutoPick = (side: StyleScheme, id: string): StyleSelection => ({
    ...baseSel,
    autoPalettes: { ...baseSel.autoPalettes, [side]: id },
  })

  /** The selection a palette card means: under Auto it fills the side on
   *  screen. Otherwise a palette the scheme can't show moves the scheme. */
  const selectionForPalette = (p: StylePaletteMeta): StyleSelection =>
    baseSel.schemeMode === 'auto'
      ? withAutoPick(osSide(), p.id)
      : {
          ...baseSel,
          paletteId: p.id,
          schemeMode: p.schemes.includes(resolvedScheme) ? baseSel.schemeMode : p.schemes[0],
        }

  const pickStyle = (style: StyleMeta): void => {
    setSelectedStyleId(style.id)
    if (style.id !== styleId) commit(selectionForStyle(style, committed), 'style')
  }

  // ── Accent ─────────────────────────────────────────────────────────
  // The hex field holds what's typed until it's a colour; a valid one
  // applies as you type, an invalid one says so when you leave the field.
  const [hexDraft, setHexDraft] = useState<string | null>(null)
  const [hexError, setHexError] = useState(false)

  const chooseAccent = (hex: string): void => {
    setAccent(hex)
    noteSave('accent', readKey(accentStorageKey(useStyleStore.getState().styleId)) === useStyleStore.getState().accent)
  }
  const handleAccentReset = (): void => {
    resetAccent()
    setHexDraft(null)
    setHexError(false)
    noteSave('accent', readKey(accentStorageKey(useStyleStore.getState().styleId)) === null)
  }
  const handleHexChange = (raw: string): void => {
    setHexDraft(raw)
    const m = HEX_RE.exec(raw.trim())
    if (m) {
      setHexError(false)
      chooseAccent(`#${m[1].toLowerCase()}`)
    }
  }
  const finishHex = (): void => {
    if (hexDraft === null) return
    if (HEX_RE.test(hexDraft.trim())) {
      setHexDraft(null)
      setHexError(false)
    } else {
      setHexError(true)
    }
  }

  // ── Dials — the active style's manifest-advertised knobs ──────────
  // Values are per-user, per-style (localStorage k2.dial.<style>.<dial>)
  // and applied as inline custom properties on <html>; the style store
  // re-stamps them on every apply, so this UI only needs to write the
  // token live + persist. Local state mirrors the values for readouts.
  const [dialValues, setDialValues] = useState<Record<string, number>>({})

  const dialValue = (dial: StyleDialMeta): number => {
    const key = dialStorageKey(selectedStyle.id, dial.id)
    if (key in dialValues) return dialValues[key]
    return resolveDialValue(dial, readKey(key) ?? null)
  }

  const handleDialInput = (dial: StyleDialMeta, value: number): void => {
    const key = dialStorageKey(selectedStyle.id, dial.id)
    document.documentElement.style.setProperty(dial.token, formatDialValue(dial, value))
    let saved = true
    try {
      localStorage.setItem(key, String(value))
    } catch {
      // Value still applies live; it just won't survive a relaunch.
      saved = false
    }
    noteSave('dials', saved)
    setDialValues((m) => ({ ...m, [key]: value }))
  }

  const handleDialReset = (dial: StyleDialMeta): void => {
    const key = dialStorageKey(selectedStyle.id, dial.id)
    const value = dialDefault(dial)
    document.documentElement.style.setProperty(dial.token, formatDialValue(dial, value))
    let saved = true
    try {
      localStorage.removeItem(key)
    } catch {
      // The live value is already back at the default.
      saved = false
    }
    noteSave('dials', saved)
    setDialValues((m) => ({ ...m, [key]: value }))
  }

  // ── Terminal text weight ───────────────────────────────────────────
  // Stored as the WebGL glyph gamma, where lower is thicker. The slider
  // runs the other way, so dragging right makes text thicker.
  const weightFromGamma = (g: number): number => Math.round((TEXT_GAMMA_MIN + TEXT_GAMMA_MAX - g) * 100) / 100
  const textWeightCustom = readStoredTextGamma(styleId, resolvedScheme) !== null
  const handleTextWeight = (weight: number): void => {
    setTextGamma(weightFromGamma(weight))
    const now = useStyleStore.getState()
    noteSave('text-weight', readStoredTextGamma(now.styleId, now.resolvedScheme) !== null)
  }
  const handleTextWeightReset = (): void => {
    resetTextGamma()
    const now = useStyleStore.getState()
    noteSave('text-weight', readKey(textGammaStorageKey(now.styleId, now.resolvedScheme)) === null)
    rerender()
  }

  const schemeDisabled = (mode: 'light' | 'dark' | 'auto'): string | null => {
    const schemes = selectedStyle.capabilities.schemes
    if (mode === 'auto') {
      return schemes.length < 2 ? `${selectedStyle.name} only has ${schemes[0] ?? 'dark'} palettes` : null
    }
    return schemes.includes(mode) ? null : `${selectedStyle.name} has no ${mode} palettes`
  }

  const showDensity = selectedStyle.capabilities.gaps
  const showDials = selectedIsActive && selectedStyle.dials.length > 0
  const layoutTitle = showDensity && showDials ? 'Layout and effects' : showDensity ? 'Layout' : 'Effects'
  const savedHere = `Saved for ${activeStyle.name} on this computer.`

  return (
    <div className="h-full min-h-0 flex">
      {/* ── The styles ── */}
      <aside
        aria-label="Styles"
        data-settings-id="styles.style-picker"
        className="w-72 flex-shrink-0 min-h-0 flex flex-col border-r border-[var(--color-border)]"
      >
        <div className="flex-shrink-0 px-4 pt-6 pb-3">
          <h2 className="flex items-center h-7 type-heading text-[var(--color-text-primary)]">Styles</h2>
          <p className="type-hint text-[var(--color-text-secondary)] leading-relaxed">
            How K2 looks on this computer. Point at a style to try it; click to use it.
          </p>
        </div>
        <div className="flex-1 min-h-0 overflow-y-auto px-4 pb-4 space-y-1 [scrollbar-gutter:stable]">
          {PICKER_STYLES.map((style) => {
            const isActive = style.id === styleId
            const isSelected = style.id === selectedStyle.id
            const swatchPalette =
              style.palettes.find((p) => p.id === (style.defaultPalettes[resolvedScheme] ?? style.defaultPalette)) ??
              style.palettes[0]
            return (
              <button
                key={style.id}
                type="button"
                data-style-row={style.id}
                data-selected={isSelected ? '' : undefined}
                aria-current={isSelected ? 'true' : undefined}
                onClick={() => pickStyle(style)}
                onMouseEnter={() => {
                  if (!isActive) preview(selectionForStyle(style, committed))
                }}
                onMouseLeave={endPreview}
                data-k2-focus=""
                className={cx(
                  'relative w-full flex flex-col gap-2 px-3 py-2.5 text-left no-drag cursor-pointer transition-colors outline-none',
                  isSelected ? 'bg-[var(--color-accent)]/15' : 'hover:bg-[var(--color-wash-1)]',
                )}
              >
                {isSelected && (
                  <span aria-hidden="true" className="absolute left-0 top-0 bottom-0 w-[3px] bg-[var(--color-accent)]" />
                )}
                <span className="flex items-center gap-2 min-w-0">
                  <span data-style-name className="type-name text-[var(--color-text-primary)] truncate flex-1">
                    {style.name}
                  </span>
                  {/* Neutral: the selected row already carries the accent. */}
                  {isActive && <Tag tone="neutral">In use</Tag>}
                </span>
                {swatchPalette && (
                  <SwatchStrip
                    className="h-4"
                    colors={[
                      swatchPalette.swatch.bg,
                      swatchPalette.swatch.surface,
                      swatchPalette.swatch.elevated,
                      swatchPalette.swatch.accent,
                      swatchPalette.swatch.textPrimary,
                    ]}
                  />
                )}
              </button>
            )
          })}
        </div>
      </aside>

      {/* ── The shown style ── */}
      <div className="flex-1 min-w-0 min-h-0 overflow-y-auto p-6 [scrollbar-gutter:stable]">
        <div className="w-full max-w-[52rem] space-y-6 pb-6">
          <header className="space-y-3">
            <div className="flex items-start justify-between gap-4">
              <div className="min-w-0">
                {/* The list marks the style in use; the title doesn't repeat it. */}
                <h2 className="type-title text-[var(--color-text-primary)] truncate">{selectedStyle.name}</h2>
                <p className="type-hint text-[var(--color-text-secondary)] mt-1 leading-relaxed">{selectedStyle.description}</p>
              </div>
              {!selectedIsActive && (
                <Button variant="accent" className="flex-shrink-0" onClick={() => pickStyle(selectedStyle)}>
                  Use {selectedStyle.name}
                </Button>
              )}
            </div>
            {unsaved === 'style' && <RowError>{NOT_SAVED}</RowError>}
          </header>

          <SettingsGroup title="Colours" hint="Changes show at once and are saved on this computer." card>
            <SettingRow
              settingId="styles.palette"
              label="Palette"
              hint={
                schemeMode === 'auto' && selectedIsActive
                  ? `Under Auto, this picks the palette for your system's ${osSide()} appearance. Point at one to try it.`
                  : 'The set of colours. Point at one to try it; click to use it.'
              }
              below={
                <div className="space-y-2">
                  <div className="flex flex-wrap gap-2">
                    {selectedStyle.palettes.map((p) => {
                      const isCurrent = selectedIsActive && p.id === resolvedPaletteId
                      const movesScheme = !p.schemes.includes(resolvedScheme) && schemeMode !== 'auto'
                      return (
                        <button
                          key={p.id}
                          type="button"
                          data-palette-id={p.id}
                          data-k2-chamfer="surface"
                          aria-pressed={isCurrent}
                          aria-label={`${p.name} palette, ${schemeWords(p).toLowerCase()}`}
                          onClick={() => commit(selectionForPalette(p), 'palette')}
                          onMouseEnter={() => preview(selectionForPalette(p))}
                          onMouseLeave={endPreview}
                          title={movesScheme ? `Switches to ${p.schemes[0]}` : undefined}
                          className={cx(
                            'k2-focus w-40 flex flex-col gap-2 p-2.5 text-left border no-drag cursor-pointer transition-colors rounded-[calc(var(--radius-box)/2)]',
                            isCurrent
                              ? 'border-[var(--color-accent-text)]'
                              : 'border-[var(--color-control-edge)] hover:border-[var(--color-text-muted)] hover:bg-[var(--color-wash-1)]',
                          )}
                        >
                          <span aria-hidden="true" className={cx('flex flex-col overflow-hidden border border-[var(--color-control-edge)]', SWATCH_SHAPE)}>
                            <span className="flex h-6">
                              {[p.swatch.bg, p.swatch.surface, p.swatch.elevated, p.swatch.accent].map((c, i) => (
                                <span key={i} className="min-w-0 flex-1" style={{ backgroundColor: c }} />
                              ))}
                            </span>
                            {/* The 16 terminal colours: what this audience judges first. */}
                            <span className="flex h-1.5">
                              {p.terminal.palette.map((n, i) => (
                                <span key={i} className="flex-1" style={{ backgroundColor: intToHex(n) }} />
                              ))}
                            </span>
                          </span>
                          <span className="block min-w-0">
                            <span className="flex items-center gap-1.5 min-w-0">
                              <span data-palette-name className="type-name text-[var(--color-text-primary)] truncate flex-1">
                                {p.name}
                              </span>
                              {isCurrent && <CheckMark />}
                            </span>
                            <span className="block type-hint text-[var(--color-text-secondary)]">{schemeWords(p)}</span>
                          </span>
                        </button>
                      )
                    })}
                  </div>
                  {unsaved === 'palette' && <RowError>{NOT_SAVED}</RowError>}
                </div>
              }
            >
              {savedNote('palette')}
            </SettingRow>

            <SettingRow
              settingId="styles.scheme"
              label="Light or dark"
              hint="Auto follows your system's appearance."
              below={unsaved === 'scheme' ? <RowError>{NOT_SAVED}</RowError> : undefined}
            >
              {savedNote('scheme')}
              {/* Manual activation: arrows only move focus, because a choice here
                  commits a style change. No value while browsing another style. */}
              <Segmented
                aria-label="Light or dark"
                activation="manual"
                value={selectedIsActive ? schemeMode : null}
                onChange={(mode) => commit({ ...baseSel, schemeMode: mode }, 'scheme')}
                options={SCHEME_OPTIONS.map((opt) => {
                  const disabledReason = schemeDisabled(opt.id)
                  return {
                    value: opt.id,
                    label: opt.label,
                    disabled: disabledReason !== null,
                    buttonProps: {
                      title: disabledReason ?? undefined,
                      onMouseEnter: () => {
                        if (!disabledReason) preview({ ...baseSel, schemeMode: opt.id })
                      },
                      onMouseLeave: endPreview,
                    },
                  }
                })}
              />
            </SettingRow>

            {/* Auto swaps between any two of the style's palettes. */}
            {schemeMode === 'auto' &&
              selectedIsActive &&
              AUTO_SIDES.map(({ side, label }, i) => (
                <SettingRow
                  key={side}
                  settingId={i === 0 ? 'styles.auto-palettes' : undefined}
                  label={label}
                  hint={i === 0 ? 'Auto uses these two palettes.' : undefined}
                  below={unsaved === 'auto' && i === AUTO_SIDES.length - 1 ? <RowError>{NOT_SAVED}</RowError> : undefined}
                >
                  {i === AUTO_SIDES.length - 1 ? savedNote('auto') : null}
                  <Segmented
                    aria-label={label}
                    activation="manual"
                    value={autoPalettes?.[side] ?? null}
                    onChange={(id) => commit(withAutoPick(side, id), 'auto')}
                    options={selectedStyle.palettes.map((p) => ({
                      value: p.id,
                      label: p.name,
                      buttonProps: {
                        onMouseEnter: () => {
                          if (side === osSide()) preview(withAutoPick(side, p.id))
                        },
                        onMouseLeave: endPreview,
                      },
                    }))}
                  />
                </SettingRow>
              ))}

            <SettingRow
              settingId="styles.accent"
              label="Accent colour"
              hint={`Buttons, switches, links and focus rings. ${savedHere}`}
              below={
                hexError ? (
                  <RowError id="styles-accent-error">
                    Use six hex digits after the #, like {accentHex}. The accent hasn&apos;t changed.
                  </RowError>
                ) : unsaved === 'accent' ? (
                  <RowError>{NOT_SAVED}</RowError>
                ) : undefined
              }
            >
              {savedNote('accent')}
              <input
                type="color"
                aria-label="Accent colour"
                data-testid="accent-well"
                value={accentHex}
                onChange={(e) => {
                  setHexDraft(null)
                  setHexError(false)
                  chooseAccent(e.target.value)
                }}
                className="k2-color-well h-6 w-8 p-0 flex-shrink-0 overflow-hidden border border-[var(--color-control-edge)] bg-transparent cursor-pointer no-drag rounded-[var(--radius-field)]"
              />
              <Input
                aria-label="Accent colour as hex"
                data-testid="accent-hex"
                spellCheck={false}
                autoComplete="off"
                maxLength={7}
                value={hexDraft ?? accentHex}
                aria-invalid={hexError || undefined}
                aria-describedby={hexError ? 'styles-accent-error' : undefined}
                onChange={(e) => handleHexChange(e.target.value)}
                onBlur={finishHex}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') finishHex()
                }}
                className={cx('w-24 font-mono', hexError && '!border-[var(--color-status-error-text)] [--k2-ch-edge:var(--color-status-error-text)]')}
              />
              <Button variant="quiet" className="flex-shrink-0" disabled={accent === null} onClick={handleAccentReset}>
                Reset
              </Button>
            </SettingRow>
          </SettingsGroup>

          {(showDensity || showDials) && (
            <SettingsGroup title={layoutTitle} card>
              {/* Density (gap presets) — only when the style declares gaps */}
              {showDensity && (
                <SettingRow
                  settingId="styles.density"
                  label="Spacing"
                  hint={`Room between panes, tiles and sections. Compact is ${selectedStyle.name}'s default.`}
                  below={unsaved === 'density' ? <RowError>{NOT_SAVED}</RowError> : undefined}
                >
                  {savedNote('density')}
                  <Segmented
                    aria-label="Spacing"
                    activation="manual"
                    value={selectedIsActive ? gapsPreset : null}
                    onChange={(preset) => commit({ ...baseSel, gapsPreset: preset }, 'density')}
                    options={densityOptions(selectedStyle).map((opt) => ({
                      value: opt.id,
                      label: opt.label,
                      buttonProps: {
                        onMouseEnter: () => preview({ ...baseSel, gapsPreset: opt.id }),
                        onMouseLeave: endPreview,
                      },
                    }))}
                  />
                </SettingRow>
              )}

              {/* Dials — manifest-advertised knobs; only for the ACTIVE style
                  (a dial tweaks the LIVE style, not a preview of another one) */}
              {showDials &&
                selectedStyle.dials.map((dial, i) => {
                  const value = dialValue(dial)
                  const custom = (readKey(dialStorageKey(selectedStyle.id, dial.id)) ?? null) !== null
                  return (
                    <SettingRow
                      key={dial.id}
                      settingId={i === 0 ? 'styles.dials' : undefined}
                      label={dial.label}
                      hint={`${DIAL_HINTS[`${selectedStyle.id}.${dial.id}`] ?? `Adjusts ${selectedStyle.name}.`} ${savedHere}`}
                      below={unsaved === 'dials' && i === selectedStyle.dials.length - 1 ? <RowError>{NOT_SAVED}</RowError> : undefined}
                    >
                      {i === selectedStyle.dials.length - 1 ? savedNote('dials') : null}
                      <Slider
                        aria-label={dial.label}
                        min={dial.min}
                        max={dial.max}
                        step={dial.step ?? 1}
                        value={value}
                        onChange={(e) => handleDialInput(dial, Number(e.target.value))}
                        className="w-40"
                      />
                      <span className={VALUE_SLOT}>{formatDialValue(dial, value)}</span>
                      <Button
                        variant="quiet"
                        className="flex-shrink-0"
                        disabled={!custom}
                        aria-label={`Reset ${dial.label}`}
                        onClick={() => handleDialReset(dial)}
                      >
                        Reset
                      </Button>
                    </SettingRow>
                  )
                })}
            </SettingsGroup>
          )}

          {/* WebGL terminal text weight — per active style + resolved scheme.
              Saved like dials (localStorage k2.textGamma.<style>.<scheme>).
              Only the ACTIVE style is editable; browsing another style's
              detail doesn't re-tune the live terminals. */}
          {selectedIsActive && (
            <SettingsGroup title="Terminals" card>
              <SettingRow
                settingId="styles.text-weight"
                label="Text weight"
                hint={
                  painter === 'webgl'
                    ? `Drag right for thicker terminal text. Saved for ${activeStyle.name} in ${resolvedScheme} mode.`
                    : 'Only works when terminals draw with WebGL, which is turned off. Turn it on in Terminal settings, under Terminal Painter.'
                }
                below={unsaved === 'text-weight' ? <RowError>{NOT_SAVED}</RowError> : undefined}
              >
                {savedNote('text-weight')}
                <Slider
                  aria-label="Terminal text weight"
                  min={TEXT_GAMMA_MIN}
                  max={TEXT_GAMMA_MAX}
                  step={0.05}
                  value={weightFromGamma(textGamma)}
                  disabled={painter !== 'webgl'}
                  onChange={(e) => handleTextWeight(parseFloat(e.target.value))}
                  className="w-40"
                />
                {/* The Frost row's value slot, empty, so both sliders end on one edge. */}
                <span aria-hidden="true" className={VALUE_SLOT} />
                {painter === 'webgl' ? (
                  <Button
                    variant="quiet"
                    className="flex-shrink-0"
                    disabled={!textWeightCustom}
                    aria-label="Reset text weight"
                    onClick={handleTextWeightReset}
                  >
                    Reset
                  </Button>
                ) : (
                  <Button variant="quiet" className="flex-shrink-0" onClick={() => openSettings?.('terminal')}>
                    Terminal settings
                  </Button>
                )}
              </SettingRow>
            </SettingsGroup>
          )}

          {/* Live preview — a miniature K2 workspace on the contract tokens
              (canvas, tab strip, tiled terminal panes using the real --term-*
              colors), so it always shows the CURRENTLY STAMPED style,
              including hover previews. */}
          <SettingsGroup title="Preview" hint="The style in use, or the one you're pointing at." settingId="styles.preview" card>
            <div className="p-4">
              <div
                data-k2-chamfer="surface"
                aria-hidden="true"
                className="border border-[var(--color-border)]"
                style={{
                  backgroundColor: 'var(--color-bg-canvas)',
                  borderRadius: 'var(--radius-box)',
                  padding: 'calc(var(--inset-window) + 8px)',
                }}
              >
                {/* Mini tab strip with status dots */}
                <div
                  className="flex items-center border border-[var(--color-border)]"
                  style={{
                    backgroundColor: 'var(--color-bg-surface)',
                    borderRadius: 'var(--radius-box)',
                    // Rounded shapes can't butt together: on a rounded style the
                    // strip floats clear of the tiles, and Terminal stays flush.
                    marginBottom: 'calc(var(--gap-section) + var(--radius-box) / 2)',
                  }}
                >
                  {['zsh', 'claude', 'logs'].map((t, i) => (
                    <div
                      key={t}
                      className="relative px-3 py-1 type-hint"
                      style={{ color: i === 0 ? 'var(--color-text-primary)' : 'var(--color-text-secondary)' }}
                    >
                      {t}
                      {/* Under the label on a rounded strip, clear of its curve;
                          the tab's full width and flush on a square one. */}
                      {i === 0 && (
                        <span
                          className="absolute h-0.5 rounded-[var(--radius-pill)]"
                          style={{
                            left: 'min(var(--radius-box), 12px)',
                            right: 'min(var(--radius-box), 12px)',
                            bottom: 'min(var(--radius-box), 2px)',
                            backgroundColor: 'var(--color-accent)',
                          }}
                        />
                      )}
                    </div>
                  ))}
                  <div className="ml-auto flex items-center gap-1.5 px-3">
                    <span className="w-1.5 h-1.5 rounded-[var(--radius-badge)]" style={{ backgroundColor: 'var(--color-status-working)' }} />
                    <span className="w-1.5 h-1.5 rounded-[var(--radius-badge)]" style={{ backgroundColor: 'var(--color-status-ok)' }} />
                  </div>
                </div>
                {/* Mini mosaic — two terminal tiles; the seam between them is
                    the canvas, exactly like the real tiling grid */}
                <div className="grid grid-cols-2 font-mono type-hint" style={{ gap: 'calc(var(--gap-tile) * 2)' }}>
                  <div
                    className="border border-[var(--color-border)] p-2 leading-relaxed"
                    style={{ backgroundColor: 'var(--term-bg)', borderRadius: 'var(--radius-box)' }}
                  >
                    <div style={{ color: 'var(--term-fg)' }}>
                      <span style={{ color: 'var(--term-ansi-10)' }}>➜</span>{' '}
                      <span style={{ color: 'var(--term-ansi-12)' }}>~/k2</span> k2 agent list
                    </div>
                    <div style={{ color: 'var(--term-ansi-2)' }}>✓ appa — ready</div>
                    <div style={{ color: 'var(--term-ansi-3)' }}>● momo — working</div>
                  </div>
                  <div
                    className="border border-[var(--color-border)] p-2 leading-relaxed"
                    style={{ backgroundColor: 'var(--term-bg)', borderRadius: 'var(--radius-box)' }}
                  >
                    <div style={{ color: 'var(--term-ansi-13)' }}>
                      <span
                        className="inline-block w-[3px] h-[10px] mr-1 align-middle rounded-[calc(var(--radius-box)/8)]"
                        style={{ backgroundColor: 'var(--term-ansi-13)' }}
                      />
                      agent
                    </div>
                    <div style={{ color: 'var(--term-fg)' }}>Refactor complete.</div>
                    <div>
                      <span style={{ color: 'var(--term-fg)' }}>$ </span>
                      <span
                        className="inline-block w-[6px] h-[10px] align-middle rounded-[calc(var(--radius-box)/8)]"
                        style={{ backgroundColor: 'var(--term-cursor)' }}
                      />
                    </div>
                  </div>
                </div>
              </div>
            </div>
          </SettingsGroup>
        </div>
      </div>
    </div>
  )
}
