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
import { STYLES, type StyleMeta, type StylePaletteMeta, type StyleScheme } from '@/styles.generated'
import {
  DAEMON_STYLE_DEFAULT,
  DEFAULT_ACCENT,
  DEFAULT_SELECTION,
  canonicalStyleSelection,
  paletteSlots,
  parseSchemeMode,
  resolveStyleSelection,
  type SchemeMode,
  type StyleSelection,
} from '@/lib/style-resolve'
import { dialStorageKey, formatDialValue, resolveDialValue } from '@/lib/style-dials'
import {
  ACCENT_VARS,
  accentStorageKey,
  accentVarsFor,
  lsAccentStampFor,
  parseAccentChoice,
  type AccentChoice,
} from '@/lib/style-accent'
import { installFrostSupport } from '@/lib/frost-support'
import { migrateRetiredStyleKeys, seedRetiredStyleAccent } from '@/lib/style-id-migration'
import {
  clampTextGamma,
  clearStoredTextGamma,
  resolveEffectiveTextGamma,
  resolveTextGammaPreset,
  writeStoredTextGamma,
} from '@/lib/text-gamma'
import { STOPLIGHT_SPACER_VAR, createTrafficLightController } from '@/lib/traffic-lights'
import { createWindowCornerController } from '@/lib/window-corners'

// ── localStorage mirror keys (dotted convention, see index.html) ─────
export const LS_STYLE = 'k2.style'
export const LS_PALETTE = 'k2.palette' // resolved palette for the CURRENT scheme
export const LS_SCHEME = 'k2.scheme' // the MODE, including 'auto'
export const LS_GAPS = 'k2.gaps'
/** One-shot upgrade flag: after `1`, daemon style is never re-applied. */
export const LS_STYLE_MIGRATED = 'k2.style.migrated'
/** One-time switch to the default look at the first launch of 0.46.11. After
 *  `1`, the style is never touched again, so a later pick sticks. index.html's
 *  bootstrap reads it too. */
export const LS_STYLE_MIGRATED_DEFAULT_GLASS = 'k2.style.migrated-0-46-11-glass'

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
  lsAccentStampFor('dark'),
  lsAccentStampFor('light'),
]

interface StyleState extends StyleSelection {
  /** schemeMode resolved against the OS appearance. */
  resolvedScheme: StyleScheme
  /** Palette id after the per-style fallback chain. */
  resolvedPaletteId: string
  /**
   * Effective WebGL text-weight gamma for the ACTIVE style/scheme.
   * Loaded from per-style localStorage (`k2.textGamma.<style>.<scheme>`)
   * or the polarity preset. Styles Settings owns the slider; open
   * WebGL panes re-read this live via PainterTheme.
   */
  textGamma: number
  /** The active style's accent choice: `yellow`, a #rrggbb colour, or
   *  null for the palette's own accent. Saved per style
   *  (`k2.accent.<styleId>`); Settings → Styles owns it. */
  accent: AccentChoice | null
  /** Resolve `sel` (merged over the current selection), stamp <html>,
   *  refresh the localStorage mirror. Idempotent. Does NOT talk to the
   *  daemon — SSOT is localStorage; callers that commit a user choice
   *  also go through settings.updateStyleSettings (local-only). */
  applyStyle: (sel: Partial<StyleSelection>) => void
  /** Persist a manual text-weight for the active style+resolved scheme
   *  and push it live into open WebGL panes. */
  setTextGamma: (v: number) => void
  /** Drop the active style+scheme override; restore polarity preset. */
  resetTextGamma: () => void
  /** Use `choice` as the active style's accent, live and saved. An invalid
   *  value is ignored. */
  setAccent: (choice: AccentChoice) => void
  /** Back to the style's own accent, at once. */
  resetAccent: () => void
}

export function osPrefersLight(): boolean {
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

/** A style's saved accent, or null (none, invalid, or storage unavailable). */
function readStoredAccent(styleId: string): AccentChoice | null {
  try {
    return parseAccentChoice(localStorage.getItem(accentStorageKey(styleId)))
  } catch {
    return null
  }
}

/** Stamp the style's saved accent as inline custom properties on <html>, over
 *  the palette's own values. Gardens are unaffected: the Zen root re-points
 *  each of these at a Zen token (lib/zen/zen-style-shield). */
function stampAccentProperties(
  style: StyleMeta,
  palette: StylePaletteMeta,
  scheme: StyleScheme,
  choice: AccentChoice | null = readStoredAccent(style.id),
): void {
  const html = document.documentElement
  for (const name of ACCENT_VARS) html.style.removeProperty(name)
  const vars = accentVarsFor(choice, palette, scheme)
  for (const [name, value] of Object.entries(vars)) html.style.setProperty(name, value)
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
  stampAccentProperties(style, resolvedPalette, resolvedScheme)
  syncTrafficLights()
  syncWindowCorners()
}

// ── macOS traffic lights follow the window inset ─────────────────────
// Floating-chrome styles (Glass/spacious presets) inset the whole
// UI from the window edge, so the close/minimize/zoom buttons must move
// down-right with it. Terminal paints those buttons as squares; every
// other style id keeps Apple's circles. AppKit resets standard-button
// frames on resize, fullscreen, and title changes, so re-apply on all
// three — including Terminal compact, whose inset is 0. Rust owns the
// re-apply (commands/traffic_lights.rs observes resize, screen, backing
// scale, wake, appearance, key/main); these hooks are the backup. The app
// zoom rides along so the buttons stay centered on the zoomed top bar, and
// every apply also writes the top-bar spacer width (--k2-stoplight-spacer)
// so the reserved gap matches the unscaled buttons at any zoom. One source:
// App.tsx calls onAppZoomChange on every zoom change and at startup.
// Fire-and-forget: in non-Tauri contexts (parity harness, plain browser)
// the invoke rejects and the miss is purely cosmetic.
// ── Native chrome owner: Style or Zen (prd-zen-mode-v1 Z23, Z49) ─────
// One owner for the window's corners and stoplights. Normally the Style
// drives them. While this window shows Zen, the Zen `[chrome]` table does:
// every existing re-apply trigger below (resize, fullscreen, zoom, a title
// change, a Style stamp or hover preview) reads the source first, so a
// Style re-apply can never stomp Zen's values mid-Zen. Rust saves the last
// values per window and re-applies those on its own AppKit observers, so
// it follows whatever this owner sent last. Setting the source back to
// 'style' re-applies the Style's values (and its stoplight spacer).
// Per window (module state per webview); macOS only, like the controllers.

export type ZenChromeSource = {
  corners: 'system' | 'square'
  /** `hidden` (the haunted Diary's theme): no window buttons while Zen shows. */
  stoplights: 'round' | 'square' | 'hidden'
  /** `[chrome] stoplight-offset`, px right and down, 0–24 each. */
  offset: [number, number]
}
export type ChromeSource = 'style' | { zen: ZenChromeSource }

let chromeSource: ChromeSource = 'style'
const chromeAppliedListeners = new Set<() => void>()

export function getChromeSource(): ChromeSource {
  return chromeSource
}

function zenChromeKey(src: ChromeSource): string {
  if (src === 'style') return 'style'
  const z = src.zen
  return `zen|${z.corners}|${z.stoplights}|${z.offset[0]}|${z.offset[1]}`
}

/**
 * Hand the window's native chrome to Zen (`{zen}`) or back to the Style
 * (`'style'`). A change re-applies at once; the same source again is a
 * no-op (so a Zen re-render never re-sends).
 */
export function setChromeSource(next: ChromeSource): void {
  if (zenChromeKey(next) === zenChromeKey(chromeSource)) return
  chromeSource = next === 'style' ? 'style' : { zen: { ...next.zen, offset: [next.zen.offset[0], next.zen.offset[1]] } }
  if (typeof document === 'undefined' || typeof navigator === 'undefined') return
  trafficLights.resetBaseline()
  trafficLights.reapply()
  windowCorners.reapply()
  notifyChromeApplied()
}

/** Called after every stoplight apply (zoom included), so Zen can recompute
 *  its stoplight safe area. Returns the unsubscribe. */
export function onChromeApplied(fn: () => void): () => void {
  chromeAppliedListeners.add(fn)
  return () => {
    chromeAppliedListeners.delete(fn)
  }
}

function notifyChromeApplied(): void {
  for (const fn of [...chromeAppliedListeners]) fn()
}

function zenTrafficLights(): { square: boolean; x: number; y: number; hidden: boolean } | null {
  if (chromeSource === 'style') return null
  const z = chromeSource.zen
  return { square: z.stoplights === 'square', x: z.offset[0], y: z.offset[1], hidden: z.stoplights === 'hidden' }
}

const trafficLights = createTrafficLightController({
  isMac: () =>
    typeof navigator !== 'undefined' && navigator.platform.toLowerCase().includes('mac'),
  read: () => {
    const el = document.documentElement
    const cs = getComputedStyle(el)
    const raw = cs.getPropertyValue('--inset-window').trim()
    return {
      styleId: el.getAttribute('data-style'),
      inset: Number.parseFloat(raw) || 0,
      barHeight: Number.parseFloat(cs.getPropertyValue('--k2-topbar-height')) || undefined,
      // App.tsx sets this with documentElement.style.zoom (Cmd+= / Cmd+-).
      zoom: (window as { __k2soZoom?: number }).__k2soZoom,
      zen: zenTrafficLights(),
    }
  },
  apply: (cmd) => {
    void invoke('set_traffic_light_inset', cmd).catch(() => {})
    notifyChromeApplied()
  },
  setSpacer: (px) => {
    document.documentElement.style.setProperty(STOPLIGHT_SPACER_VAR, `${px}px`)
  },
  schedule: (fn) => {
    requestAnimationFrame(fn)
  },
})

function syncTrafficLights(): void {
  if (typeof document === 'undefined' || typeof navigator === 'undefined') return
  trafficLights.syncIfChanged()
}

/**
 * The app zoom changed (or the app started). Re-centers the native buttons
 * on the zoomed top bar and resizes the spacer. macOS only; a no-op on
 * Linux, Windows, and other platforms.
 */
export function onAppZoomChange(): void {
  if (typeof document === 'undefined' || typeof navigator === 'undefined') return
  trafficLights.onZoomChange()
}

/** AppKit puts the buttons back at the top when the window title changes. */
export function reapplyTrafficLights(): void {
  if (typeof document === 'undefined' || typeof navigator === 'undefined') return
  trafficLights.reapply()
}

// Terminal's window clip. Not the traffic-light inset: compact still
// reapplies. Hover stamps this window only (no localStorage write).
// 0.5 for `terminal`, 0 for every other id.
const windowCorners = createWindowCornerController({
  isMac: () =>
    typeof navigator !== 'undefined' && navigator.platform.toLowerCase().includes('mac'),
  read: () => {
    const el = document.documentElement
    return {
      styleId: el.getAttribute('data-style'),
      scheme: el.getAttribute('data-scheme'),
      palette: el.getAttribute('data-palette'),
      gaps: el.getAttribute('data-gaps'),
      zenCorners: chromeSource === 'style' ? null : chromeSource.zen.corners,
    }
  },
  apply: (radius) => {
    void invoke('set_window_corner_radius', { radius }).catch(() => {})
  },
  schedule: (fn) => {
    requestAnimationFrame(fn)
  },
})

function syncWindowCorners(): void {
  if (typeof document === 'undefined' || typeof navigator === 'undefined') return
  windowCorners.reapply()
}

/** AppKit rewrites the system squircle across fullscreen and setTitle. */
export function reapplyWindowCorners(): void {
  if (typeof document === 'undefined' || typeof navigator === 'undefined') return
  windowCorners.reapply()
}

if (typeof window !== 'undefined') {
  // Re-apply after this module loads. A hot update must re-execute this
  // file; otherwise the native buttons keep the inset from launch.
  if (import.meta.hot) import.meta.hot.accept()
  queueMicrotask(() => {
    trafficLights.resetBaseline()
    syncTrafficLights()
    syncWindowCorners()
  })
  window.addEventListener('resize', () => {
    trafficLights.onResize()
    windowCorners.onResize()
  })
  // WKWebView does not emit this for the green button. Rust re-applies
  // traffic lights and the corner radius on
  // NSWindowDidEnter/ExitFullScreenNotification for that path.
  if (typeof document !== 'undefined') {
    document.addEventListener('fullscreenchange', () => {
      trafficLights.onFullscreen()
      windowCorners.onFullscreen()
    })
    document.addEventListener('webkitfullscreenchange', () => {
      trafficLights.onFullscreen()
      windowCorners.onFullscreen()
    })
  }
}

/** The accent's pre-paint stamp, one per OS appearance so a flip while the
 *  app was closed still paints the right values. */
function writeAccentStamps(sel: StyleSelection): void {
  if (typeof localStorage === 'undefined') return
  try {
    for (const side of ['dark', 'light'] as const) {
      const key = lsAccentStampFor(side)
      const { style, resolvedPalette, resolvedScheme } = resolveStyleSelection(sel, side === 'light')
      const vars = accentVarsFor(readStoredAccent(style.id), resolvedPalette, resolvedScheme)
      if (Object.keys(vars).length === 0) localStorage.removeItem(key)
      else localStorage.setItem(key, JSON.stringify({ style: style.id, palette: resolvedPalette.id, vars }))
    }
  } catch {
    // Only costs the pre-paint stamp; the live <html> values are right.
  }
}

function writeMirror(sel: StyleSelection): void {
  if (typeof localStorage === 'undefined') return
  try {
    const resolved = resolveStyleSelection(sel, osPrefersLight())
    localStorage.setItem(LS_STYLE, resolved.style.id)
    localStorage.setItem(LS_PALETTE, resolved.resolvedPalette.id)
    localStorage.setItem(LS_SCHEME, sel.schemeMode)
    localStorage.setItem(LS_GAPS, resolved.gapsPreset)
    const slots = paletteSlots(resolved.style, sel)
    localStorage.setItem(lsPaletteFor('dark'), slots.dark)
    localStorage.setItem(lsPaletteFor('light'), slots.light)
  } catch {
    // Quota/privacy failures only cost the pre-paint mirror; the live
    // store + <html> attributes still reflect the selection.
  }
  writeAccentStamps(sel)
}

/** A fresh copy of the default look (its Auto picks are shared otherwise). */
function defaultSelection(): StyleSelection {
  return { ...DEFAULT_SELECTION, autoPalettes: { ...DEFAULT_SELECTION.autoPalettes } }
}

/** True when this client has no saved style at all: it gets the default
 *  look. index.html's bootstrap makes the same test (`k2.style` empty). */
export function hasNoSavedStyle(): boolean {
  if (typeof localStorage === 'undefined') return false
  try {
    return !localStorage.getItem(LS_STYLE)
  } catch {
    return false
  }
}

/** Best-effort boot selection from the localStorage mirror. With no saved
 *  style, the default look whatever else is there. */
export function readMirror(): StyleSelection {
  if (typeof localStorage === 'undefined') return defaultSelection()
  try {
    if (!localStorage.getItem(LS_STYLE)) return defaultSelection()
    const dark = localStorage.getItem(lsPaletteFor('dark'))
    const light = localStorage.getItem(lsPaletteFor('light'))
    return canonicalStyleSelection({
      styleId: localStorage.getItem(LS_STYLE) ?? DEFAULT_SELECTION.styleId,
      paletteId: localStorage.getItem(LS_PALETTE) ?? DEFAULT_SELECTION.paletteId,
      schemeMode: parseSchemeMode(localStorage.getItem(LS_SCHEME)),
      gapsPreset: localStorage.getItem(LS_GAPS) ?? DEFAULT_SELECTION.gapsPreset,
      ...(dark !== null || light !== null
        ? { autoPalettes: { ...(dark !== null ? { dark } : {}), ...(light !== null ? { light } : {}) } }
        : {}),
    })
  } catch {
    return defaultSelection()
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
 * Seed the default look's accent on a client with no saved style, before the
 * first read of the mirror. Only when the default style has no accent saved:
 * an empty key keeps meaning the palette's own accent for everyone else.
 * Returns whether it wrote one. Never throws.
 */
function seedDefaultAccent(): boolean {
  if (!hasNoSavedStyle()) return false
  try {
    const key = accentStorageKey(DEFAULT_SELECTION.styleId)
    if (localStorage.getItem(key) !== null) return false
    localStorage.setItem(key, DEFAULT_ACCENT)
    return localStorage.getItem(key) === DEFAULT_ACCENT
  } catch {
    return false
  }
}

/** Whether this boot gave the default style the default accent. */
let _seededDefaultAccent = false

/**
 * The one-time switch to the default look (Rosson, 2026-11-11): every client,
 * new or existing, starts 0.46.11 on Glass under Auto (Obsidian dark, Veil
 * light) with the Yellow accent, once. Runs before the first read of the
 * mirror; index.html's bootstrap paints the same look while the flag is
 * missing, so the first frame is already Glass.
 *
 * The flag is written first and read back: if it can't be saved the switch
 * doesn't happen at all, so a client whose storage refuses writes is never
 * switched on every launch. It also marks the older daemon migration done:
 * a daemon's style echo must never undo the switch. The daemon never holds
 * the style (settings.updateStyleSettings is local-only), so there is
 * nothing to send it. Returns whether it switched. Never throws.
 */
function migrateToDefaultLook(): boolean {
  if (typeof localStorage === 'undefined') return false
  try {
    if (localStorage.getItem(LS_STYLE_MIGRATED_DEFAULT_GLASS) === '1') return false
    localStorage.setItem(LS_STYLE_MIGRATED_DEFAULT_GLASS, '1')
    if (localStorage.getItem(LS_STYLE_MIGRATED_DEFAULT_GLASS) !== '1') return false
  } catch {
    return false
  }
  try {
    const sel = defaultSelection()
    const resolved = resolveStyleSelection(sel, osPrefersLight())
    localStorage.setItem(LS_STYLE, sel.styleId)
    localStorage.setItem(LS_PALETTE, resolved.resolvedPalette.id)
    localStorage.setItem(LS_SCHEME, sel.schemeMode)
    localStorage.setItem(LS_GAPS, sel.gapsPreset)
    localStorage.setItem(lsPaletteFor('dark'), sel.autoPalettes!.dark!)
    localStorage.setItem(lsPaletteFor('light'), sel.autoPalettes!.light!)
    localStorage.setItem(accentStorageKey(sel.styleId), DEFAULT_ACCENT)
    localStorage.setItem(LS_STYLE_MIGRATED, '1')
  } catch {
    // Part-written: the store reads what landed and re-stamps the mirror.
  }
  return true
}

/**
 * One-shot upgrade migration: seed localStorage from a daemon style
 * echo only when this install has no prior local selection.
 *
 * Rules (in order):
 *  1. Already migrated → no-op forever
 *  2. Pre-boot mirror was complete → mark migrated, keep local
 *  3. Daemon returned a style someone chose → apply once to local, mark
 *     migrated. The daemon's own default (square / charcoal / dark, what
 *     every daemon echoes until set) is not a choice: it never replaces the
 *     default look on a new client.
 *  4. Else → mark migrated (defaults already live from boot)
 */
export function migrateStyleFromDaemon(daemonStyle: DaemonStyleSeed | undefined | null): void {
  if (isStyleMigrated()) return

  if (_preBootMirrorComplete) {
    markStyleMigrated()
    return
  }

  if (daemonStyle && hasDaemonStylePayload(daemonStyle) && !isDaemonDefaultStyle(daemonStyle)) {
    // The default accent came with the default look this boot; the
    // person's own look comes with the palette's accent, as it did.
    if (_seededDefaultAccent) {
      try {
        const key = accentStorageKey(DEFAULT_SELECTION.styleId)
        if (localStorage.getItem(key) === DEFAULT_ACCENT) localStorage.removeItem(key)
      } catch {
        // They keep the default accent on that style.
      }
      _seededDefaultAccent = false
    }
    if (daemonStyle.id) seedRetiredStyleAccent(daemonStyle.id, parseSchemeMode(daemonStyle.scheme))
    const sel = canonicalStyleSelection({
      styleId: daemonStyle.id ?? DAEMON_STYLE_DEFAULT.id,
      paletteId: daemonStyle.palette ?? DAEMON_STYLE_DEFAULT.palette,
      schemeMode: parseSchemeMode(daemonStyle.scheme),
      gapsPreset: daemonStyle.gaps ?? DAEMON_STYLE_DEFAULT.gaps,
    })
    useStyleStore.getState().applyStyle(sel)
    markStyleMigrated()
    return
  }

  markStyleMigrated()
}

/** The daemon's never-set default, under its old or new id. */
function isDaemonDefaultStyle(s: DaemonStyleSeed): boolean {
  return (
    (s.id == null || s.id === DAEMON_STYLE_DEFAULT.id || s.id === 'terminal') &&
    (s.palette == null || s.palette === DAEMON_STYLE_DEFAULT.palette) &&
    (s.scheme == null || s.scheme === DAEMON_STYLE_DEFAULT.scheme) &&
    (s.gaps == null || s.gaps === DAEMON_STYLE_DEFAULT.gaps)
  )
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

// Before the first read of the mirror, so the store and every per-style
// key lookup after it only ever see current ids.
migrateRetiredStyleKeys()
migrateToDefaultLook()
_seededDefaultAccent = seedDefaultAccent()

const bootSelection = readMirror()
const bootResolved = resolveStyleSelection(bootSelection, osPrefersLight())
const bootTextGamma = resolveEffectiveTextGamma(
  bootResolved.style,
  bootResolved.resolvedPalette,
  bootResolved.resolvedScheme,
)

/** The current selection, for re-stamping the accent after a change. */
function selectionOf(s: StyleSelection): StyleSelection {
  return {
    styleId: s.styleId,
    paletteId: s.paletteId,
    schemeMode: s.schemeMode,
    gapsPreset: s.gapsPreset,
    autoPalettes: s.autoPalettes,
  }
}

/** Write the accent onto <html> and the pre-paint stamp, without the rest of
 *  a style stamp (a colour drag shouldn't re-apply the window chrome). The
 *  choice is passed in, so it shows even when storage refused to save it. */
function applyAccent(sel: StyleSelection, choice: AccentChoice | null): void {
  if (typeof document === 'undefined') return
  const { style, resolvedPalette, resolvedScheme } = resolveStyleSelection(sel, osPrefersLight())
  stampAccentProperties(style, resolvedPalette, resolvedScheme, choice)
  writeAccentStamps(sel)
}

export const useStyleStore = create<StyleState>((set, get) => ({
  ...bootSelection,
  resolvedScheme: bootResolved.resolvedScheme,
  resolvedPaletteId: bootResolved.resolvedPalette.id,
  textGamma: bootTextGamma,
  accent: readStoredAccent(bootSelection.styleId),

  applyStyle: (partial: Partial<StyleSelection>) => {
    const cur = get()
    // When the resolved style identity changes (style / palette / scheme),
    // load THAT identity's stored textGamma or its polarity preset.
    // Same-identity re-apply (boot no-op, multi-window restamp with no
    // change) leaves the live textGamma alone so a mid-session slider
    // value isn't clobbered before it's written — setTextGamma owns writes.
    const prevResolvedKey = `${cur.styleId}\0${cur.resolvedPaletteId}\0${cur.resolvedScheme}`
    const canonical = canonicalStyleSelection({
      styleId: partial.styleId ?? cur.styleId,
      paletteId: partial.paletteId ?? cur.paletteId,
      schemeMode: partial.schemeMode ?? cur.schemeMode,
      gapsPreset: partial.gapsPreset ?? cur.gapsPreset,
      autoPalettes: partial.autoPalettes ?? cur.autoPalettes,
    })
    const resolved = resolveStyleSelection(canonical, osPrefersLight())
    const sel: StyleSelection = { ...canonical, autoPalettes: paletteSlots(resolved.style, canonical) }
    const nextResolvedKey = `${sel.styleId}\0${resolved.resolvedPalette.id}\0${resolved.resolvedScheme}`
    const next: Partial<StyleState> = {
      ...sel,
      resolvedScheme: resolved.resolvedScheme,
      resolvedPaletteId: resolved.resolvedPalette.id,
      accent: readStoredAccent(sel.styleId),
    }
    if (prevResolvedKey !== nextResolvedKey) {
      next.textGamma = resolveEffectiveTextGamma(
        resolved.style,
        resolved.resolvedPalette,
        resolved.resolvedScheme,
      )
    }
    set(next)
    stampStyleAttributes(sel)
    writeMirror(sel)
  },

  setTextGamma: (v: number) => {
    const cur = get()
    const clamped = clampTextGamma(v)
    writeStoredTextGamma(cur.styleId, cur.resolvedScheme, clamped)
    set({ textGamma: clamped })
  },

  resetTextGamma: () => {
    const cur = get()
    clearStoredTextGamma(cur.styleId, cur.resolvedScheme)
    const resolved = resolveStyleSelection(
      {
        styleId: cur.styleId,
        paletteId: cur.paletteId,
        schemeMode: cur.schemeMode,
        gapsPreset: cur.gapsPreset,
      },
      osPrefersLight(),
    )
    set({
      textGamma: resolveTextGammaPreset(resolved.style, resolved.resolvedPalette),
    })
  },

  setAccent: (choice: AccentChoice) => {
    const parsed = parseAccentChoice(choice)
    if (parsed === null) return
    const sel = selectionOf(get())
    try {
      localStorage.setItem(accentStorageKey(sel.styleId), parsed)
    } catch {
      // Still applies below for this session; it just won't survive a relaunch.
    }
    set({ accent: parsed })
    applyAccent(sel, parsed)
  },

  resetAccent: () => {
    const sel = selectionOf(get())
    try {
      localStorage.removeItem(accentStorageKey(sel.styleId))
    } catch {
      // Best-effort; the live values below are already the default.
    }
    set({ accent: null })
    applyAccent(sel, null)
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
  installFrostSupport()

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
