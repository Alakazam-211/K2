// Settings → Styles (Style System P3, prd-style-system-v1 §5 / ST5).
//
// Master-detail (same shell idiom as ProjectsSection): style cards on
// the left; the selected style's palette grid, scheme control, density
// control, and a live preview strip on the right.
//
// Interaction contract:
// - HOVER previews are strictly attribute-level (stampStyleAttributes)
//   with no store/state/persistence writes, so a stray hover can never
//   race the daemon round-trip; mouse-leave restores the committed
//   selection's attributes.
// - CLICK commits: applyStyle (store + <html> + localStorage mirror)
//   then updateStyleSettings (daemon persistence + cross-window sync).

import React, { useCallback, useState } from 'react'
import { useSettingsStore } from '@/stores/settings'
import { stampStyleAttributes, useStyleStore } from '@/stores/style'
import type { StyleSelection } from '@/lib/style-resolve'
import {
  STYLES,
  type StyleMeta,
  type StylePaletteMeta,
  type StyleScheme,
} from '@/styles.generated'
import type { SettingEntry } from '../searchManifest'

export const STYLES_MANIFEST: SettingEntry[] = [
  { id: 'styles.style-picker', section: 'styles', label: 'Style', description: 'App-wide visual style (Square, community styles)', keywords: ['style', 'skin', 'theme', 'appearance', 'look', 'square', 'glass', 'bezel'] },
  { id: 'styles.palette', section: 'styles', label: 'Palette', description: 'Color palette within the selected style', keywords: ['palette', 'color', 'colors', 'theme', 'charcoal', 'paper', 'dark mode', 'light mode'] },
  { id: 'styles.scheme', section: 'styles', label: 'Scheme', description: 'Light / Dark / Auto — Auto follows the OS appearance', keywords: ['scheme', 'light', 'dark', 'auto', 'appearance', 'mode', 'night', 'day', 'os'] },
  { id: 'styles.density', section: 'styles', label: 'Density', description: 'Compact / Regular / Spacious gaps between panes and tiles', keywords: ['density', 'gaps', 'spacing', 'compact', 'regular', 'spacious', 'padding', 'tiles'] },
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
                  <div className="flex gap-1 mb-2">
                    {([p.swatch.bg, p.swatch.surface, p.swatch.elevated, p.swatch.accent] as const).map((c, i) => (
                      <span
                        key={i}
                        className="w-5 h-5 border border-[var(--color-border)] flex-shrink-0"
                        style={{ backgroundColor: c }}
                      />
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

        {/* Live preview strip — pure divs on the contract tokens, so it
            always shows the CURRENTLY STAMPED style (incl. hover previews). */}
        <div className="mb-6" data-settings-id="styles.preview">
          <div className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider mb-2">
            Preview
          </div>
          <div
            className="p-4 border border-[var(--color-border)] max-w-xl"
            style={{ backgroundColor: 'var(--color-bg)', borderRadius: 'var(--radius-box)' }}
          >
            {/* 3-tab strip */}
            <div className="flex border-b border-[var(--color-border)] mb-3">
              {['terminal', 'chat', 'files'].map((t, i) => (
                <div
                  key={t}
                  className={`px-3 py-1 text-[11px] border-r border-[var(--color-border)] ${
                    i === 0
                      ? 'bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)]'
                      : 'text-[var(--color-text-muted)]'
                  }`}
                  style={{ borderTopLeftRadius: 'var(--radius-selector)', borderTopRightRadius: 'var(--radius-selector)' }}
                >
                  {t}
                </div>
              ))}
            </div>
            <div className="flex gap-3">
              {/* Surface card */}
              <div
                className="flex-1 p-3 border border-[var(--color-border)]"
                style={{ backgroundColor: 'var(--color-bg-surface)', borderRadius: 'var(--radius-box)' }}
              >
                <div className="text-[11px] text-[var(--color-text-primary)] mb-1">Surface</div>
                <div className="text-[10px] text-[var(--color-text-secondary)]">Secondary text</div>
                <div className="text-[10px] text-[var(--color-text-muted)]">Muted text</div>
              </div>
              {/* Elevated card + controls */}
              <div
                className="flex-1 p-3 border border-[var(--color-border)]"
                style={{ backgroundColor: 'var(--color-bg-elevated)', borderRadius: 'var(--radius-box)' }}
              >
                <div className="text-[11px] text-[var(--color-text-primary)] mb-2">Elevated</div>
                <div className="flex items-center gap-2">
                  <div
                    className="px-2.5 py-1 text-[10px]"
                    style={{
                      backgroundColor: 'var(--color-accent)',
                      color: 'var(--color-on-accent)',
                      borderRadius: 'var(--radius-field)',
                    }}
                  >
                    Button
                  </div>
                  <div
                    className="px-2 py-1 text-[10px] border border-[var(--color-border)] text-[var(--color-text-muted)] flex-1 min-w-0"
                    style={{ backgroundColor: 'var(--color-bg)', borderRadius: 'var(--radius-field)' }}
                  >
                    Input…
                  </div>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  )
}
