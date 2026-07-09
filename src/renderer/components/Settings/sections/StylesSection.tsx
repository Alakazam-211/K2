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
import { stampStyleAttributes, useStyleStore } from '@/stores/style'
import type { StyleSelection } from '@/lib/style-resolve'
import { dialDefault, dialStorageKey, formatDialValue, resolveDialValue } from '@/lib/style-dials'
import {
  STYLES,
  type StyleDialMeta,
  type StyleMeta,
  type StylePaletteMeta,
  type StyleScheme,
} from '@/styles.generated'
import type { SettingEntry } from '../searchManifest'

/** Registry terminal colors are 0xRRGGBB ints (Kessel's format). */
const intToHex = (n: number): string => `#${n.toString(16).padStart(6, '0')}`

export const STYLES_MANIFEST: SettingEntry[] = [
  { id: 'styles.style-picker', section: 'styles', label: 'Style', description: 'App-wide visual style (Square, community styles)', keywords: ['style', 'skin', 'theme', 'appearance', 'look', 'square', 'glass', 'bezel'] },
  { id: 'styles.palette', section: 'styles', label: 'Palette', description: 'Color palette within the selected style', keywords: ['palette', 'color', 'colors', 'theme', 'charcoal', 'paper', 'dark mode', 'light mode'] },
  { id: 'styles.scheme', section: 'styles', label: 'Scheme', description: 'Light / Dark / Auto — Auto follows the OS appearance', keywords: ['scheme', 'light', 'dark', 'auto', 'appearance', 'mode', 'night', 'day', 'os'] },
  { id: 'styles.density', section: 'styles', label: 'Density', description: 'Compact / Regular / Spacious gaps between panes and tiles', keywords: ['density', 'gaps', 'spacing', 'compact', 'regular', 'spacious', 'padding', 'tiles'] },
  { id: 'styles.dials', section: 'styles', label: 'Dials', description: 'Style-specific adjustments the active style advertises (e.g. Glass frost)', keywords: ['dials', 'dial', 'knobs', 'frost', 'blur', 'adjust', 'tweak', 'slider', 'glass'] },
]

const SCHEME_OPTIONS: { id: 'light' | 'dark' | 'auto'; label: string }[] = [
  { id: 'light', label: 'Light' },
  { id: 'dark', label: 'Dark' },
  { id: 'auto', label: 'Auto' },
]

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
  const resolvedScheme = useStyleStore((s) => s.resolvedScheme)
  const resolvedPaletteId = useStyleStore((s) => s.resolvedPaletteId)
  const applyStyle = useStyleStore((s) => s.applyStyle)
  const updateStyleSettings = useSettingsStore((s) => s.updateStyleSettings)

  const committed: StyleSelection = { styleId, paletteId, schemeMode, gapsPreset }

  // Which style's DETAIL is showing. Defaults to the active style; kept
  // as local state so browsing a style's palettes doesn't commit it.
  const [selectedStyleId, setSelectedStyleId] = useState(styleId)
  const selectedStyle = STYLES.find((s) => s.id === selectedStyleId) ?? STYLES[0]
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
  }, [styleId, paletteId, schemeMode, gapsPreset])

  // Base selection edits in the detail pane operate on: the committed
  // selection when the active style is showing, else a jump-in default.
  const baseSel = selectedIsActive ? committed : selectionForStyle(selectedStyle, committed)

  const handlePaletteClick = (p: StylePaletteMeta): void => {
    // Least-surprise: clicking a palette that can't render in the current
    // scheme switches the scheme mode to one it supports.
    const nextMode = p.schemes.includes(resolvedScheme) ? baseSel.schemeMode : p.schemes[0]
    commit({ ...baseSel, paletteId: p.id, schemeMode: nextMode })
  }

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
          {STYLES.map((style) => {
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
                <div className="flex gap-1 mt-2">
                  {swatchPalette &&
                    ([
                      swatchPalette.swatch.bg,
                      swatchPalette.swatch.surface,
                      swatchPalette.swatch.elevated,
                      swatchPalette.swatch.accent,
                      swatchPalette.swatch.textPrimary,
                    ] as const).map((c, i) => (
                      <span
                        key={i}
                        className="w-4 h-4 border border-[var(--color-border)] flex-shrink-0"
                        style={{ backgroundColor: c }}
                      />
                    ))}
                </div>
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
              const schemeCaption = !supportsScheme
                ? p.schemes.length === 1
                  ? `${capitalize(p.schemes[0])} only`
                  : p.schemes.map(capitalize).join(' / ')
                : null
              return (
                <button
                  key={p.id}
                  onClick={() => handlePaletteClick(p)}
                  onMouseEnter={() =>
                    preview({
                      ...baseSel,
                      paletteId: p.id,
                      schemeMode: supportsScheme ? baseSel.schemeMode : p.schemes[0],
                    })
                  }
                  onMouseLeave={endPreview}
                  title={schemeCaption ? `Switches the scheme to ${p.schemes[0]}` : undefined}
                  className={`w-40 text-left p-2.5 border no-drag cursor-pointer transition-colors ${
                    isCurrent
                      ? 'border-[var(--color-accent)] bg-[var(--color-bg-elevated)]'
                      : 'border-[var(--color-border)] bg-[var(--color-bg-surface)] hover:bg-[var(--color-bg-elevated)]'
                  } ${!supportsScheme ? 'opacity-50' : ''}`}
                >
                  <div className="flex gap-1 mb-1.5">
                    {([p.swatch.bg, p.swatch.surface, p.swatch.elevated, p.swatch.accent] as const).map((c, i) => (
                      <span
                        key={i}
                        className="w-5 h-5 border border-[var(--color-border)] flex-shrink-0"
                        style={{ backgroundColor: c }}
                      />
                    ))}
                  </div>
                  {/* ANSI-16 strip — terminal colors are what this audience judges first */}
                  <div className="flex mb-2 border border-[var(--color-border)]">
                    {p.terminal.palette.map((n, i) => (
                      <span key={i} className="h-1.5 flex-1" style={{ backgroundColor: intToHex(n) }} />
                    ))}
                  </div>
                  <div className="flex items-center justify-between">
                    <span className="text-[11px] text-[var(--color-text-primary)]">{p.name}</span>
                    {schemeCaption && (
                      <span className="text-[9px] text-[var(--color-text-muted)]">{schemeCaption}</span>
                    )}
                  </div>
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
          <div className="inline-flex border border-[var(--color-border)]">
            {SCHEME_OPTIONS.map((opt) => {
              const disabledReason = schemeDisabled(opt.id)
              const isCurrent = selectedIsActive && schemeMode === opt.id
              return (
                <button
                  key={opt.id}
                  disabled={disabledReason !== null}
                  title={disabledReason ?? undefined}
                  onClick={() => handleSchemeClick(opt.id)}
                  onMouseEnter={() => {
                    if (!disabledReason) preview({ ...baseSel, schemeMode: opt.id })
                  }}
                  onMouseLeave={endPreview}
                  className={`px-3 py-1 text-[11px] no-drag transition-colors ${
                    isCurrent
                      ? 'bg-[var(--color-accent)] text-[var(--color-on-accent)]'
                      : disabledReason
                        ? 'text-[var(--color-text-muted)] opacity-50 cursor-not-allowed'
                        : 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)] cursor-pointer'
                  }`}
                >
                  {opt.label}
                </button>
              )
            })}
          </div>
          {schemeMode === 'auto' && selectedIsActive && (
            <div className="text-[10px] text-[var(--color-text-muted)] mt-1.5">
              Following the OS appearance — currently {resolvedScheme}.
            </div>
          )}
        </div>

        {/* Density (gap presets) — only when the style declares gaps */}
        {selectedStyle.capabilities.gaps && (
          <div className="mb-6" data-settings-id="styles.density">
            <div className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider mb-2">
              Density
            </div>
            <div className="inline-flex border border-[var(--color-border)]">
              {densityOptions(selectedStyle).map((opt) => {
                const isCurrent = selectedIsActive && gapsPreset === opt.id
                return (
                  <button
                    key={opt.id || 'base'}
                    onClick={() => handleDensityClick(opt.id)}
                    onMouseEnter={() => preview({ ...baseSel, gapsPreset: opt.id })}
                    onMouseLeave={endPreview}
                    className={`px-3 py-1 text-[11px] no-drag cursor-pointer transition-colors ${
                      isCurrent
                        ? 'bg-[var(--color-accent)] text-[var(--color-on-accent)]'
                        : 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)]'
                    }`}
                  >
                    {opt.label}
                  </button>
                )
              })}
            </div>
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
                    <input
                      type="range"
                      min={dial.min}
                      max={dial.max}
                      step={dial.step ?? 1}
                      value={value}
                      onChange={(e) => handleDialInput(dial, Number(e.target.value))}
                      className="w-40 no-drag k2so-slider"
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

        {/* Live preview — a miniature K2 workspace on the contract tokens
            (canvas, tab strip, tiled terminal panes using the real --term-*
            colors), so it always shows the CURRENTLY STAMPED style,
            including hover previews. */}
        <div className="mb-6" data-settings-id="styles.preview">
          <div className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider mb-2">
            Preview
          </div>
          <div
            className="border border-[var(--color-border)] max-w-xl overflow-hidden"
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
                borderTopLeftRadius: 'var(--radius-box)',
                borderTopRightRadius: 'var(--radius-box)',
                marginBottom: 'var(--gap-section)',
              }}
            >
              {['zsh', 'claude', 'logs'].map((t, i) => (
                <div
                  key={t}
                  className="px-3 py-1 text-[10px]"
                  style={
                    i === 0
                      ? { color: 'var(--color-text-primary)', boxShadow: 'inset 0 -2px 0 0 var(--color-accent)' }
                      : { color: 'var(--color-text-muted)' }
                  }
                >
                  {t}
                </div>
              ))}
              <div className="ml-auto flex items-center gap-1.5 px-3">
                <span className="w-1.5 h-1.5 rounded-full" style={{ backgroundColor: 'var(--color-status-working)' }} />
                <span className="w-1.5 h-1.5 rounded-full" style={{ backgroundColor: 'var(--color-status-ok)' }} />
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
                <div className="text-[9px]" style={{ color: 'var(--term-ansi-13)' }}>▍agent</div>
                <div className="text-[9px]" style={{ color: 'var(--term-fg)' }}>Refactor complete.</div>
                <div className="text-[9px]">
                  <span style={{ color: 'var(--term-fg)' }}>$ </span>
                  <span
                    className="inline-block w-[6px] h-[10px] align-middle"
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
