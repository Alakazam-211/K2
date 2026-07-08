// Style System P3 — the renderer's live Style selection.
//
// Owns the (styleId, paletteId, schemeMode, gapsPreset) selection plus
// its resolved derivatives, and is the ONLY writer of the <html>
// data-style/data-palette/data-scheme/data-gaps attributes after first
// paint (index.html's inline bootstrap stamps them pre-paint from the
// localStorage mirror this store maintains).
//
// Persistence is daemon-canonical and lives in stores/settings.ts
// (`updateStyleSettings` / the fetchSettings read-back, which calls
// `applyStyle` here so the daemon's value wins over the mirror).
// IMPORTANT: this module must NOT import stores/settings.ts — settings
// fires a daemon fetch at module-init and imports us for the read-back
// apply; importing it back would both cycle and defeat the
// ConnectionGate's deferred-import contract.

import { create } from 'zustand'
import { STYLES, type StyleMeta, type StyleScheme } from '@/styles.generated'
import {
  DEFAULT_SELECTION,
  parseSchemeMode,
  resolvePalette,
  resolveStyleSelection,
  type SchemeMode,
  type StyleSelection,
} from '@/lib/style-resolve'
import { dialStorageKey, formatDialValue, resolveDialValue } from '@/lib/style-dials'

// ── localStorage mirror keys (dotted convention, see index.html) ─────
const LS_STYLE = 'k2.style'
const LS_PALETTE = 'k2.palette' // resolved palette for the CURRENT scheme
const LS_SCHEME = 'k2.scheme' // the MODE, including 'auto'
const LS_GAPS = 'k2.gaps'
/** Per-scheme resolved palette (`k2.palette.dark` / `k2.palette.light`)
 *  so the pre-paint bootstrap is right even after an OS appearance flip
 *  while the app was closed (mode 'auto' resolves to a different scheme
 *  than the one `k2.palette` was written under). */
const lsPaletteFor = (scheme: StyleScheme): string => `${LS_PALETTE}.${scheme}`

interface StyleState extends StyleSelection {
  /** schemeMode resolved against the OS appearance. */
  resolvedScheme: StyleScheme
  /** Palette id after the per-style fallback chain. */
  resolvedPaletteId: string
  /** Resolve `sel` (merged over the current selection), stamp <html>,
   *  refresh the localStorage mirror. Idempotent. Does NOT persist —
   *  callers that commit go through settings.updateStyleSettings. */
  applyStyle: (sel: Partial<StyleSelection>) => void
}

function osPrefersLight(): boolean {
  return (
    typeof window !== 'undefined' &&
    typeof window.matchMedia === 'function' &&
    window.matchMedia('(prefers-color-scheme: light)').matches
  )
}

/** Stamp the style's dial tokens as inline custom properties on <html>.
 *  Every dial token ANY style declares is cleared first (the registry
 *  makes the full list enumerable), so switching styles never leaves a
 *  stale dial value behind. Values come from the localStorage
 *  `k2.dial.<styleId>.<dialId>` keys, falling back to each dial's
 *  declared default (clamped — see lib/style-dials). */
function stampDialProperties(style: StyleMeta): void {
  const html = document.documentElement
  for (const s of STYLES) {
    for (const d of s.dials) html.style.removeProperty(d.token)
  }
  for (const dial of style.dials) {
    let raw: string | null = null
    try {
      raw = localStorage.getItem(dialStorageKey(style.id, dial.id))
    } catch {
      // Privacy-mode storage failure → the dial rests at its default.
    }
    html.style.setProperty(dial.token, formatDialValue(dial, resolveDialValue(dial, raw)))
  }
}

/** Stamp the resolved selection onto <html>. Exported for the Settings
 *  page's hover-preview, which is deliberately attribute-level only
 *  (no store/state/persistence writes) so a stray hover can never race
 *  the daemon round-trip. */
export function stampStyleAttributes(sel: StyleSelection): void {
  if (typeof document === 'undefined') return
  const { style, resolvedScheme, resolvedPalette, gapsPreset } = resolveStyleSelection(
    sel,
    osPrefersLight(),
  )
  const html = document.documentElement
  html.setAttribute('data-style', style.id)
  html.setAttribute('data-palette', resolvedPalette.id)
  html.setAttribute('data-scheme', resolvedScheme)
  if (gapsPreset) html.setAttribute('data-gaps', gapsPreset)
  else html.removeAttribute('data-gaps')
  stampDialProperties(style)
}

function writeMirror(sel: StyleSelection): void {
  if (typeof localStorage === 'undefined') return
  try {
    const resolved = resolveStyleSelection(sel, osPrefersLight())
    localStorage.setItem(LS_STYLE, resolved.style.id)
    localStorage.setItem(LS_PALETTE, resolved.resolvedPalette.id)
    localStorage.setItem(LS_SCHEME, sel.schemeMode)
    localStorage.setItem(LS_GAPS, resolved.gapsPreset)
    for (const scheme of ['dark', 'light'] as const) {
      localStorage.setItem(
        lsPaletteFor(scheme),
        resolvePalette(resolved.style, sel.paletteId, scheme).id,
      )
    }
  } catch {
    // Quota/privacy failures only cost the pre-paint mirror; the daemon
    // read-back re-applies the canonical selection right after boot.
  }
}

/** Best-effort boot selection from the localStorage mirror. `k2.palette`
 *  holds the RESOLVED palette (close enough as the "chosen" one until
 *  the daemon read-back corrects it a beat later). */
function readMirror(): StyleSelection {
  if (typeof localStorage === 'undefined') return { ...DEFAULT_SELECTION }
  try {
    return {
      styleId: localStorage.getItem(LS_STYLE) ?? DEFAULT_SELECTION.styleId,
      paletteId: localStorage.getItem(LS_PALETTE) ?? DEFAULT_SELECTION.paletteId,
      schemeMode: parseSchemeMode(localStorage.getItem(LS_SCHEME)),
      gapsPreset: localStorage.getItem(LS_GAPS) ?? DEFAULT_SELECTION.gapsPreset,
    }
  } catch {
    return { ...DEFAULT_SELECTION }
  }
}

const bootSelection = readMirror()
const bootResolved = resolveStyleSelection(bootSelection, osPrefersLight())

export const useStyleStore = create<StyleState>((set, get) => ({
  ...bootSelection,
  resolvedScheme: bootResolved.resolvedScheme,
  resolvedPaletteId: bootResolved.resolvedPalette.id,

  applyStyle: (partial: Partial<StyleSelection>) => {
    const cur = get()
    const sel: StyleSelection = {
      styleId: partial.styleId ?? cur.styleId,
      paletteId: partial.paletteId ?? cur.paletteId,
      schemeMode: partial.schemeMode ?? cur.schemeMode,
      gapsPreset: partial.gapsPreset ?? cur.gapsPreset,
    }
    const resolved = resolveStyleSelection(sel, osPrefersLight())
    set({
      ...sel,
      resolvedScheme: resolved.resolvedScheme,
      resolvedPaletteId: resolved.resolvedPalette.id,
    })
    stampStyleAttributes(sel)
    writeMirror(sel)
  },
}))

// ── Boot + auto-scheme listener (module init runs once per window) ───
if (typeof window !== 'undefined') {
  // Attributes were already stamped pre-paint by index.html's bootstrap;
  // re-applying makes the store state, mirror (incl. the per-scheme
  // keys) and any resolution fallback consistent with it. The daemon
  // fetch in stores/settings.ts corrects this moments later.
  useStyleStore.getState().applyStyle({})

  // Live OS-appearance listener: only acts when the MODE is 'auto'
  // (explicit dark/light selections must not move with the OS).
  // Installed once at module scope — repeated applyStyle calls or
  // Settings visits never add another listener.
  if (typeof window.matchMedia === 'function') {
    const mq = window.matchMedia('(prefers-color-scheme: dark)')
    const onChange = (): void => {
      const s = useStyleStore.getState()
      if (s.schemeMode === 'auto') s.applyStyle({})
    }
    if (typeof mq.addEventListener === 'function') mq.addEventListener('change', onChange)
    // Legacy WebKit fallback — addListener is deprecated but still the
    // only seam on older webviews.
    else if (typeof mq.addListener === 'function') mq.addListener(onChange)
  }
}
