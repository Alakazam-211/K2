// Style System — the renderer's live Style selection (per-client view state).
//
// Owns the (styleId, paletteId, schemeMode, gapsPreset) selection plus
// its resolved derivatives, and is the ONLY writer of the <html>
// data-style/data-palette/data-scheme/data-gaps attributes after first
// paint (index.html's inline bootstrap stamps them pre-paint from the
// localStorage mirror this store maintains).
//
// SSOT is localStorage on THIS install (thin-client view state). The
// daemon may still echo a legacy `style` key for back-compat, but it is
// NEVER authority after the one-shot migration (`k2.style.migrated`).
// Host switch / fetchSettings must not restyle from the daemon.
// `settings.updateStyleSettings` writes the mirror only (no daemon POST).
// IMPORTANT: this module must NOT import stores/settings.ts — settings
// fires a daemon fetch at module-init and imports us for migration;
// importing it back would both cycle and defeat the ConnectionGate's
// deferred-import contract.

import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
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
export const LS_STYLE = 'k2.style'
export const LS_PALETTE = 'k2.palette' // resolved palette for the CURRENT scheme
export const LS_SCHEME = 'k2.scheme' // the MODE, including 'auto'
export const LS_GAPS = 'k2.gaps'
/** One-shot upgrade flag: after `1`, daemon style is never re-applied. */
export const LS_STYLE_MIGRATED = 'k2.style.migrated'

/** Per-scheme resolved palette (`k2.palette.dark` / `k2.palette.light`)
 *  so the pre-paint bootstrap is right even after an OS appearance flip
 *  while the app was closed (mode 'auto' resolves to a different scheme
 *  than the one `k2.palette` was written under). */
export const lsPaletteFor = (scheme: StyleScheme): string => `${LS_PALETTE}.${scheme}`

/** Keys that peer windows write — a `storage` event on any of these
 *  restamps this window from the local mirror (multi-window sync). */
export const STYLE_MIRROR_STORAGE_KEYS: readonly string[] = [
  LS_STYLE,
  LS_PALETTE,
  LS_SCHEME,
  LS_GAPS,
  lsPaletteFor('dark'),
  lsPaletteFor('light'),
]

interface StyleState extends StyleSelection {
  /** schemeMode resolved against the OS appearance. */
  resolvedScheme: StyleScheme
  /** Palette id after the per-style fallback chain. */
  resolvedPaletteId: string
  /** Resolve `sel` (merged over the current selection), stamp <html>,
   *  refresh the localStorage mirror. Idempotent. Does NOT talk to the
   *  daemon — SSOT is localStorage; callers that commit a user choice
   *  also go through settings.updateStyleSettings (local-only). */
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
 *  a commit. */
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
  syncTrafficLights()
}

// ── macOS traffic lights follow the window inset ─────────────────────
// Floating-chrome styles (Glass/Bezel/spacious presets) inset the whole
// UI from the window edge, so the close/minimize/zoom buttons must move
// down-right with it. AppKit resets standard-button frames on resize and
// fullscreen transitions, so we also re-apply on window resize.
// Fire-and-forget: in non-Tauri contexts (parity harness, plain browser)
// the invoke rejects and the miss is purely cosmetic.
let lastTrafficInset = -1

function applyTrafficInset(inset: number): void {
  void invoke('set_traffic_light_inset', { x: inset, y: inset }).catch(() => {})
}

function syncTrafficLights(): void {
  if (typeof document === 'undefined' || typeof navigator === 'undefined') return
  if (!navigator.platform.toLowerCase().includes('mac')) return
  const raw = getComputedStyle(document.documentElement).getPropertyValue('--inset-window').trim()
  const inset = Number.parseFloat(raw) || 0
  if (inset === lastTrafficInset) return
  lastTrafficInset = inset
  applyTrafficInset(inset)
}

if (typeof window !== 'undefined') {
  let queued = false
  window.addEventListener('resize', () => {
    if (queued || lastTrafficInset <= 0) return
    queued = true
    requestAnimationFrame(() => {
      queued = false
      applyTrafficInset(lastTrafficInset)
    })
  })
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
    // Quota/privacy failures only cost the pre-paint mirror; the live
    // store + <html> attributes still reflect the selection.
  }
}

/** Best-effort boot selection from the localStorage mirror. */
export function readMirror(): StyleSelection {
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

/** True when the install already has a style + scheme selection in
 *  localStorage (the PRD "mirror complete" check). */
export function isMirrorComplete(): boolean {
  if (typeof localStorage === 'undefined') return false
  try {
    return localStorage.getItem(LS_STYLE) != null && localStorage.getItem(LS_SCHEME) != null
  } catch {
    return false
  }
}

export function isStyleMigrated(): boolean {
  if (typeof localStorage === 'undefined') return false
  try {
    return localStorage.getItem(LS_STYLE_MIGRATED) === '1'
  } catch {
    return false
  }
}

export function markStyleMigrated(): void {
  if (typeof localStorage === 'undefined') return
  try {
    localStorage.setItem(LS_STYLE_MIGRATED, '1')
  } catch {
    // Privacy-mode: migration will re-run next session; harmless.
  }
}

/**
 * Snapshot whether the install already had a local style selection
 * BEFORE this process's boot `applyStyle` stamped defaults into the
 * mirror. Boot always writes the resolved selection (so the next
 * pre-paint bootstrap is warm); without this snapshot the one-shot
 * migration would treat those boot-stamped defaults as "user already
 * chose locally" and never seed from a daemon that still holds the
 * pre-upgrade selection.
 *
 * `let` (not const) so unit tests can simulate empty vs complete
 * pre-boot mirrors without re-importing the module.
 */
let _preBootMirrorComplete: boolean =
  typeof localStorage !== 'undefined' ? isMirrorComplete() : false

/** Test/introspection helper — pre-boot completeness (captured at import). */
export function wasPreBootMirrorComplete(): boolean {
  return _preBootMirrorComplete
}

/** Test-only: override the pre-boot mirror snapshot. */
export function __setPreBootMirrorCompleteForTests(complete: boolean): void {
  _preBootMirrorComplete = complete
}

/** Daemon-shaped style payload (matches `StyleSettingsBackend` fields). */
export interface DaemonStyleSeed {
  id?: string
  palette?: string
  scheme?: string
  gaps?: string
}

/**
 * One-shot upgrade migration: seed localStorage from a daemon style
 * echo only when this install has no prior local selection.
 *
 * Rules (in order):
 *  1. Already migrated → no-op forever
 *  2. Pre-boot mirror was complete → mark migrated, keep local
 *  3. Daemon returned a style → apply once to local, mark migrated
 *  4. Else → mark migrated (defaults already live from boot)
 */
export function migrateStyleFromDaemon(daemonStyle: DaemonStyleSeed | undefined | null): void {
  if (isStyleMigrated()) return

  if (_preBootMirrorComplete) {
    markStyleMigrated()
    return
  }

  if (daemonStyle && hasDaemonStylePayload(daemonStyle)) {
    const sel: StyleSelection = {
      styleId: daemonStyle.id ?? DEFAULT_SELECTION.styleId,
      paletteId: daemonStyle.palette ?? DEFAULT_SELECTION.paletteId,
      schemeMode: parseSchemeMode(daemonStyle.scheme),
      gapsPreset: daemonStyle.gaps ?? DEFAULT_SELECTION.gapsPreset,
    }
    useStyleStore.getState().applyStyle(sel)
    markStyleMigrated()
    return
  }

  markStyleMigrated()
}

function hasDaemonStylePayload(s: DaemonStyleSeed): boolean {
  return s.id != null || s.palette != null || s.scheme != null || s.gaps != null
}

/** Map live style-store state into the settings-store shape. */
export function styleSelectionToBackend(sel: {
  styleId: string
  paletteId: string
  schemeMode: SchemeMode
  gapsPreset: string
}): { id: string; palette: string; scheme: string; gaps: string } {
  return {
    id: sel.styleId,
    palette: sel.paletteId,
    scheme: sel.schemeMode,
    gaps: sel.gapsPreset,
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
  // keys) and any resolution fallback consistent with it. Daemon style
  // is only consulted once via migrateStyleFromDaemon on first settings
  // load when the pre-boot mirror was empty.
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

  // Multi-window local sync: peer windows write the same localStorage
  // keys; the browser fires `storage` only in *other* documents. Re-read
  // the mirror and restamp — never take style authority from
  // daemon `sync:settings` / fetchSettings.
  const mirrorKeySet = new Set<string>(STYLE_MIRROR_STORAGE_KEYS)
  window.addEventListener('storage', (e: StorageEvent) => {
    if (e.storageArea && e.storageArea !== localStorage) return
    // `key === null` means clear(); re-apply defaults from empty mirror.
    if (e.key !== null && !mirrorKeySet.has(e.key)) return
    restampFromLocalMirror()
  })
}

/** Re-read the localStorage mirror and applyStyle. Used by the multi-
 *  window `storage` listener; exported for unit tests. */
export function restampFromLocalMirror(): void {
  useStyleStore.getState().applyStyle(readMirror())
}
