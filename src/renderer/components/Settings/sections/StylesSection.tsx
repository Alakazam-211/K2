// Settings → Styles (Style System P3, prd-style-system-v1 §5 / ST5).
//
// Master-detail (same shell idiom as ProjectsSection): style cards on
// the left; the selected style's palette grid, scheme control, density
// control, and a live preview strip on the right.
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

import React, { useCallback, useState } from 'react'
import { useSettingsStore } from '@/stores/settings'
import { osPrefersLight, stampStyleAttributes, useStyleStore } from '@/stores/style'
import type { StyleSelection } from '@/lib/style-resolve'
import { dialDefault, dialStorageKey, formatDialValue, resolveDialValue } from '@/lib/style-dials'
import { accentFillHex } from '@/lib/style-accent'
import { TEXT_GAMMA_MAX, TEXT_GAMMA_MIN } from '@/lib/text-gamma'
import {
  STYLES,
  type StyleDialMeta,
  type StyleMeta,
  type StylePaletteMeta,
  type StyleScheme,
} from '@/styles.generated'
import type { SettingEntry } from '../searchManifest'
import { Button, Segmented, Slider } from '@/components/ui'

/** Registry terminal colors are 0xRRGGBB ints (Kessel's format). */
const intToHex = (n: number): string => `#${n.toString(16).padStart(6, '0')}`

export const STYLES_MANIFEST: SettingEntry[] = [
  { id: 'styles.style-picker', section: 'styles', label: 'Style', description: 'App-wide visual style (Terminal, Glass, community styles)', keywords: ['style', 'skin', 'theme', 'appearance', 'look', 'terminal', 'square', 'glass'] },
  { id: 'styles.palette', section: 'styles', label: 'Palette', description: 'Color palette within the selected style', keywords: ['palette', 'color', 'colors', 'theme', 'charcoal', 'paper', 'dark mode', 'light mode'] },
  { id: 'styles.scheme', section: 'styles', label: 'Scheme', description: 'Light / Dark / Auto — Auto follows the OS appearance', keywords: ['scheme', 'light', 'dark', 'auto', 'appearance', 'mode', 'night', 'day', 'os'] },
  { id: 'styles.auto-palettes', section: 'styles', label: 'Auto colour themes', description: 'Under Auto, the colour theme for a light system and for a dark system', keywords: ['auto', 'system', 'light', 'dark', 'swap', 'palette', 'theme', 'yellow'] },
  { id: 'styles.accent', section: 'styles', label: 'Accent', description: 'Your own accent colour for this style, with a reset to the theme default', keywords: ['accent', 'colour', 'color', 'tint', 'highlight', 'custom', 'reset', 'picker'] },
  { id: 'styles.density', section: 'styles', label: 'Density', description: 'Compact / Regular / Spacious gaps between panes and tiles', keywords: ['density', 'gaps', 'spacing', 'compact', 'regular', 'spacious', 'padding', 'tiles'] },
  { id: 'styles.dials', section: 'styles', label: 'Dials', description: 'Style-specific adjustments the active style advertises (e.g. Glass frost)', keywords: ['dials', 'dial', 'knobs', 'frost', 'blur', 'adjust', 'tweak', 'slider', 'glass'] },
  { id: 'styles.text-weight', section: 'styles', label: 'Terminal text weight (WebGL)', description: 'Per-style WebGL glyph weight. Lower = thicker. Saved per style and scheme.', keywords: ['gamma', 'weight', 'thickness', 'bold', 'thin', 'chonky', 'webgl', 'text', 'terminal'] },
]

/** Retired looks stay compiled but never reach the picker. */
const PICKER_STYLES = STYLES.filter((s) => !s.hidden)

const AUTO_SIDES: { side: StyleScheme; label: string }[] = [
  { side: 'light', label: 'When my system is light' },
  { side: 'dark', label: 'When my system is dark' },
]

const SCHEME_OPTIONS: { id: 'light' | 'dark' | 'auto'; label: string }[] = [
  { id: 'light', label: 'Light' },
  { id: 'dark', label: 'Dark' },
  { id: 'auto', label: 'Auto' },
]

/** Small shapes inside a card: square on Terminal; on Glass a third of the
 *  card's radius, so they sit concentric within its padding. */
const SWATCH_SHAPE = 'rounded-[calc(var(--radius-box)/3)]'

/** A row of colour chips sharing the card's inner width evenly. */
function SwatchRow({ colors, className }: { colors: readonly string[]; className: string }): React.JSX.Element {
  return (
    <div className={`flex gap-1 ${className}`}>
      {colors.map((c, i) => (
        <span
          key={i}
          className={`h-5 min-w-0 flex-1 border border-[var(--color-border)] ${SWATCH_SHAPE}`}
          style={{ backgroundColor: c }}
        />
      ))}
    </div>
  )
}

function capitalize(s: string): string {
  return s.length ? s[0].toUpperCase() + s.slice(1) : s
}

/** Density options: '' is every style's base (flush/compact) density,
 *  then whatever gap presets the style declares, in declared order. */
function densityOptions(style: StyleMeta): { id: string; label: string }[] {
  return [{ id: '', label: 'Compact' }, ...style.gapPresets.map((p) => ({ id: p, label: capitalize(p) }))]
}

/** The default selection when jumping INTO a style (style card click /
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

  const committed: StyleSelection = { styleId, paletteId, schemeMode, gapsPreset, autoPalettes }

  // The accent belongs to the style in use, whichever card's detail is showing.
  const activeStyle = STYLES.find((s) => s.id === styleId) ?? PICKER_STYLES[0]
  const activePalette = activeStyle.palettes.find((p) => p.id === resolvedPaletteId) ?? activeStyle.palettes[0]
  const accentHex = accentFillHex(accent, activePalette, resolvedScheme)

  // Which style's DETAIL is showing. Defaults to the active style; kept
  // as local state so browsing a style's palettes doesn't commit it.
  const [selectedStyleId, setSelectedStyleId] = useState(styleId)
  const selectedStyle = PICKER_STYLES.find((s) => s.id === selectedStyleId) ?? PICKER_STYLES[0]
  const selectedIsActive = selectedStyle.id === styleId

  const commit = useCallback(
    (sel: StyleSelection): void => {
      applyStyle(sel) // instant: store + <html> + mirror
      void updateStyleSettings({
        id: sel.styleId,
        palette: sel.paletteId,
        scheme: sel.schemeMode,
        gaps: sel.gapsPreset,
      })
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

  const handlePaletteClick = (p: StylePaletteMeta): void => commit(selectionForPalette(p))

  const handleSchemeClick = (mode: 'light' | 'dark' | 'auto'): void => {
    commit({ ...baseSel, schemeMode: mode })
  }

  const handleDensityClick = (preset: string): void => {
    commit({ ...baseSel, gapsPreset: preset })
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
    let raw: string | null = null
    try {
      raw = localStorage.getItem(key)
    } catch {
      // Privacy-mode storage failure → rest at the default.
    }
    return resolveDialValue(dial, raw)
  }

  const handleDialInput = (dial: StyleDialMeta, value: number): void => {
    const key = dialStorageKey(selectedStyle.id, dial.id)
    document.documentElement.style.setProperty(dial.token, formatDialValue(dial, value))
    try {
      localStorage.setItem(key, String(value))
    } catch {
      // Value still applies live; it just won't survive a relaunch.
    }
    setDialValues((m) => ({ ...m, [key]: value }))
  }

  const handleDialReset = (dial: StyleDialMeta): void => {
    const key = dialStorageKey(selectedStyle.id, dial.id)
    const value = dialDefault(dial)
    document.documentElement.style.setProperty(dial.token, formatDialValue(dial, value))
    try {
      localStorage.removeItem(key)
    } catch {
      // Best-effort; the live value is already back at the default.
    }
    setDialValues((m) => ({ ...m, [key]: value }))
  }

  const schemeDisabled = (mode: 'light' | 'dark' | 'auto'): string | null => {
    const schemes = selectedStyle.capabilities.schemes
    if (mode === 'auto') {
      return schemes.length < 2
        ? `${selectedStyle.name} only supports ${schemes[0] ?? 'dark'} palettes`
        : null
    }
    return schemes.includes(mode) ? null : `${selectedStyle.name} has no ${mode} palettes`
  }

  return (
    <div className="flex h-full min-h-0">
      {/* ── Left (master): style cards ── */}
      <div className="w-60 flex-shrink-0 border-r border-[var(--color-border)] flex flex-col">
        <div className="px-3 pt-3 pb-2 border-b border-[var(--color-border)]">
          <span className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">
            Styles
          </span>
        </div>
        <div className="flex-1 overflow-y-auto p-2 flex flex-col gap-2" data-settings-id="styles.style-picker">
          {PICKER_STYLES.map((style) => {
            const isActive = style.id === styleId
            const isSelected = style.id === selectedStyleId
            const swatchPalette =
              style.palettes.find((p) => p.id === (style.defaultPalettes[resolvedScheme] ?? style.defaultPalette)) ??
              style.palettes[0]
            return (
              <button
                key={style.id}
                onClick={() => {
                  setSelectedStyleId(style.id)
                  if (!isActive) commit(selectionForStyle(style, committed))
                }}
                onMouseEnter={() => {
                  if (!isActive) preview(selectionForStyle(style, committed))
                }}
                onMouseLeave={endPreview}
                className={`text-left p-3 border no-drag cursor-pointer transition-colors ${
                  isActive
                    ? 'border-[var(--color-accent)] bg-[var(--color-bg-elevated)]'
                    : isSelected
                      ? 'border-[var(--color-border-strong)] bg-[var(--color-bg-surface)]'
                      : 'border-[var(--color-border)] bg-[var(--color-bg-surface)] hover:bg-[var(--color-bg-elevated)]'
                }`}
              >
                <div className="flex items-center justify-between">
                  <span className="text-xs font-medium text-[var(--color-text-primary)]">{style.name}</span>
                  {isActive && (
                    <span className="text-[9px] uppercase tracking-wider text-[var(--color-accent)]">Active</span>
                  )}
                </div>
                <div className="text-[10px] text-[var(--color-text-muted)] mt-0.5">by {style.author}</div>
                {/* Mini swatch strip — 5 chips from the style's default palette */}
                {swatchPalette && (
                  <SwatchRow
                    className="mt-2"
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
      </div>

      {/* ── Right (detail): palettes / scheme / density / preview ── */}
      <div className="flex-1 min-w-0 overflow-y-auto p-6">
        <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-1">{selectedStyle.name}</h2>
        <p className="text-[11px] text-[var(--color-text-muted)] mb-5 max-w-xl">{selectedStyle.description}</p>

        {/* Palette grid */}
        <div className="mb-6" data-settings-id="styles.palette">
          <div className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider mb-2">
            Palette
          </div>
          <div className="flex flex-wrap gap-2">
            {selectedStyle.palettes.map((p) => {
              const isCurrent = selectedIsActive && p.id === resolvedPaletteId
              const supportsScheme = p.schemes.includes(resolvedScheme)
              return (
                <button
                  key={p.id}
                  onClick={() => handlePaletteClick(p)}
                  onMouseEnter={() => preview(selectionForPalette(p))}
                  onMouseLeave={endPreview}
                  title={!supportsScheme && schemeMode !== 'auto' ? `Switches the scheme to ${p.schemes[0]}` : undefined}
                  className={`w-40 text-left p-2.5 border no-drag cursor-pointer transition-colors ${
                    isCurrent
                      ? 'border-[var(--color-accent)] bg-[var(--color-bg-elevated)]'
                      : 'border-[var(--color-border)] bg-[var(--color-bg-surface)] hover:bg-[var(--color-bg-elevated)]'
                  }`}
                >
                  <SwatchRow className="mb-1.5" colors={[p.swatch.bg, p.swatch.surface, p.swatch.elevated, p.swatch.accent]} />
                  {/* ANSI-16 strip — terminal colors are what this audience judges first */}
                  <div className={`flex mb-2 overflow-hidden border border-[var(--color-border)] ${SWATCH_SHAPE}`}>
                    {p.terminal.palette.map((n, i) => (
                      <span key={i} className="h-1.5 flex-1" style={{ backgroundColor: intToHex(n) }} />
                    ))}
                  </div>
                  <span className="text-[11px] text-[var(--color-text-primary)]">{p.name}</span>
                </button>
              )
            })}
          </div>
        </div>

        {/* Scheme segmented control */}
        <div className="mb-6" data-settings-id="styles.scheme">
          <div className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider mb-2">
            Scheme
          </div>
          {/* Manual activation: arrows only move focus, because a choice here
              commits a style change. No value while browsing another style. */}
          <Segmented
            aria-label="Scheme"
            activation="manual"
            value={selectedIsActive ? schemeMode : null}
            onChange={handleSchemeClick}
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
          {/* Auto swaps between any two of the style's colour themes. */}
          {schemeMode === 'auto' && selectedIsActive && (
            <div className="mt-3 flex flex-col gap-2" data-settings-id="styles.auto-palettes">
              {AUTO_SIDES.map(({ side, label }) => (
                <div key={side} className="flex items-center gap-3 flex-wrap">
                  <span className="w-40 text-[11px] text-[var(--color-text-secondary)]">{label}</span>
                  <Segmented
                    aria-label={label}
                    activation="manual"
                    value={autoPalettes?.[side] ?? null}
                    onChange={(id) => commit(withAutoPick(side, id))}
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
                </div>
              ))}
            </div>
          )}
        </div>

        {/* Accent: the theme's own, or the person's colour, for the style in use */}
        <div className="mb-6" data-settings-id="styles.accent">
          <div className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider mb-2">
            Accent
          </div>
          <div className="flex items-center gap-3 flex-wrap">
            <input
              type="color"
              aria-label="Accent colour"
              data-testid="accent-well"
              value={accentHex}
              onChange={(e) => setAccent(e.target.value)}
              className="k2-color-well h-6 w-8 p-0 border border-[var(--color-border)] bg-transparent cursor-pointer no-drag"
            />
            <span className="text-[11px] font-mono text-[var(--color-text-muted)]">{accentHex}</span>
            <Button variant="quiet" disabled={accent === null} onClick={resetAccent}>
              Reset to default
            </Button>
          </div>
          <div className="text-[10px] text-[var(--color-text-muted)] mt-1.5">
            Buttons, switches, focus rings and links use this colour. Saved for {activeStyle.name} on this machine.
          </div>
        </div>

        {/* Density (gap presets) — only when the style declares gaps */}
        {selectedStyle.capabilities.gaps && (
          <div className="mb-6" data-settings-id="styles.density">
            <div className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider mb-2">
              Density
            </div>
            <Segmented
              aria-label="Density"
              activation="manual"
              value={selectedIsActive ? gapsPreset : null}
              onChange={handleDensityClick}
              options={densityOptions(selectedStyle).map((opt) => ({
                value: opt.id,
                label: opt.label,
                buttonProps: {
                  onMouseEnter: () => preview({ ...baseSel, gapsPreset: opt.id }),
                  onMouseLeave: endPreview,
                },
              }))}
            />
            <div className="text-[10px] text-[var(--color-text-muted)] mt-1.5">
              Gaps between panes, tiles and sections. Compact is {selectedStyle.name}&apos;s flush default.
            </div>
          </div>
        )}

        {/* Dials — manifest-advertised knobs; only for the ACTIVE style
            (a dial tweaks the LIVE style, not a preview of another one) */}
        {selectedIsActive && selectedStyle.dials.length > 0 && (
          <div className="mb-6" data-settings-id="styles.dials">
            <div className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider mb-2">
              Dials
            </div>
            <div className="flex flex-col gap-2">
              {selectedStyle.dials.map((dial) => {
                const value = dialValue(dial)
                return (
                  <div key={dial.id} className="flex items-center gap-3">
                    <span className="w-20 text-[11px] text-[var(--color-text-secondary)]">{dial.label}</span>
                    <Slider
                      min={dial.min}
                      max={dial.max}
                      step={dial.step ?? 1}
                      value={value}
                      onChange={(e) => handleDialInput(dial, Number(e.target.value))}
                      className="w-40"
                    />
                    <span className="w-12 text-[11px] text-[var(--color-text-muted)] tabular-nums">
                      {formatDialValue(dial, value)}
                    </span>
                    <button
                      onClick={() => handleDialReset(dial)}
                      className="text-[10px] no-drag cursor-pointer text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors"
                    >
                      Reset
                    </button>
                  </div>
                )
              })}
            </div>
            <div className="text-[10px] text-[var(--color-text-muted)] mt-1.5">
              {selectedStyle.name}&apos;s own adjustments — saved on this machine.
            </div>
          </div>
        )}

        {/* WebGL terminal text weight — per active style + resolved scheme.
            Saved like dials (localStorage k2.textGamma.<style>.<scheme>).
            Only the ACTIVE style is editable; browsing another style's
            detail doesn't re-tune the live painter. */}
        {selectedIsActive && (
          <div className="mb-6" data-settings-id="styles.text-weight">
            <div className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider mb-2">
              Terminal text weight (WebGL)
            </div>
            <div className="flex items-center gap-3 flex-wrap">
              <Slider
                min={TEXT_GAMMA_MIN}
                max={TEXT_GAMMA_MAX}
                step={0.05}
                value={textGamma}
                onChange={(e) => setTextGamma(parseFloat(e.target.value))}
                className="w-40"
              />
              <input
                type="number"
                min={TEXT_GAMMA_MIN}
                max={TEXT_GAMMA_MAX}
                step={0.05}
                value={Number(textGamma.toFixed(2))}
                onChange={(e) => {
                  const v = parseFloat(e.target.value)
                  if (Number.isFinite(v)) setTextGamma(v)
                }}
                className="w-16 px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag text-center"
              />
              <button
                type="button"
                onClick={() => resetTextGamma()}
                className="text-[10px] no-drag cursor-pointer text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors"
                title="Restore the polarity default for this style (dark ≈ 0.7, light ≈ 1.05)"
              >
                Reset
              </button>
            </div>
            <div className="text-[10px] text-[var(--color-text-muted)] mt-1.5">
              Only affects the WebGL painter (Settings → Terminal → Painter = WebGL;
              open a <em>new</em> tab after switching). Lower = thicker. Live — open
              WebGL terminals repaint as you drag. Saved for {selectedStyle.name} /{' '}
              {resolvedScheme} on this machine.
            </div>
          </div>
        )}

        {/* Live preview — a miniature K2 workspace on the contract tokens
            (canvas, tab strip, tiled terminal panes using the real --term-*
            colors), so it always shows the CURRENTLY STAMPED style,
            including hover previews. */}
        <div className="mb-6" data-settings-id="styles.preview">
          <div className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider mb-2">
            Preview
          </div>
          <div
            data-k2-chamfer="surface"
            className="border border-[var(--color-border)] max-w-xl"
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
                  className="relative px-3 py-1 text-[10px]"
                  style={{ color: i === 0 ? 'var(--color-text-primary)' : 'var(--color-text-muted)' }}
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
            <div className="grid grid-cols-2" style={{ gap: 'calc(var(--gap-tile) * 2)' }}>
              <div
                className="border border-[var(--color-border)] p-2 leading-relaxed"
                style={{ backgroundColor: 'var(--term-bg)', borderRadius: 'var(--radius-box)' }}
              >
                <div className="text-[9px]" style={{ color: 'var(--term-fg)' }}>
                  <span style={{ color: 'var(--term-ansi-10)' }}>➜</span>{' '}
                  <span style={{ color: 'var(--term-ansi-12)' }}>~/k2</span> k2 agent list
                </div>
                <div className="text-[9px]" style={{ color: 'var(--term-ansi-2)' }}>✓ appa — ready</div>
                <div className="text-[9px]" style={{ color: 'var(--term-ansi-3)' }}>● momo — working</div>
              </div>
              <div
                className="border border-[var(--color-border)] p-2 leading-relaxed"
                style={{ backgroundColor: 'var(--term-bg)', borderRadius: 'var(--radius-box)' }}
              >
                <div className="text-[9px]" style={{ color: 'var(--term-ansi-13)' }}>
                  <span
                    className="inline-block w-[3px] h-[10px] mr-1 align-middle rounded-[calc(var(--radius-box)/8)]"
                    style={{ backgroundColor: 'var(--term-ansi-13)' }}
                  />
                  agent
                </div>
                <div className="text-[9px]" style={{ color: 'var(--term-fg)' }}>Refactor complete.</div>
                <div className="text-[9px]">
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
      </div>
    </div>
  )
}
