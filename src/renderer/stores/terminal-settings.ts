import { create } from 'zustand'
import { persist, createJSONStorage } from 'zustand/middleware'
import {
  TERMINAL_FONT_SIZE_MIN,
  TERMINAL_FONT_SIZE_MAX,
  TERMINAL_FONT_SIZE_DEFAULT
} from '../../shared/constants'
import {
  TEXT_GAMMA_LIGHT,
  clampTextGamma,
} from '../lib/text-gamma'

export type LinkClickMode = 'click' | 'cmd-click'
export type ShortcutModifierLayout = 'cmd-active-cmdshift-pinned' | 'cmd-pinned-cmdshift-active'

/**
 * Terminal rendering backend selection.
 *
 * - `kessel` (canonical; the stack shipped as the default under its
 *   working name `alacritty-v2` since 0.36.7): daemon-hosted PTY +
 *   alacritty_terminal::Term. Tauri is a pure viewer rendering
 *   daemon-pushed grid snapshots + deltas. Session survives Tauri
 *   quit; heartbeats can target it. Labeled "Alacritty" in the UI.
 *   The A1–A5 phase plan from `.k2so/prds/alacritty-v2.md` landed
 *   in 0.34–0.36; the stack is now production-hardened and is
 *   officially named Kessel (an un-renaming — Kessel was the R&D
 *   project that seeded it).
 * - `alacritty-v2` (legacy alias): the pre-rename working name for
 *   the Kessel stack. Persisted values get coerced to `kessel` by
 *   the v5 migration below; already-open tabs stamped with it still
 *   dispatch to the Kessel pane.
 * - `alacritty` (legacy): in-process alacritty_terminal engine + DOM
 *   renderer. PTY lives in the Tauri process; session dies with the
 *   app. Removed from the Settings UI in 0.37.0; the setter and
 *   persist migration both coerce it to `kessel`. The legacy
 *   Rust spawn path remains compiled in to host any pre-existing
 *   in-flight tabs gracefully and is slated for removal in a later
 *   release.
 *
 * Changes to this setting only affect NEW tabs; already-open tabs
 * keep their chosen renderer. Zustand's persist middleware means
 * existing users keep whatever they had set — only fresh installs
 * see the new `kessel` default.
 *
 * 0.39.0: The experimental `kessel` JSON-stream renderer was
 * retired; any persisted value got coerced to `alacritty-v2` on
 * load via the v3 migration below. (The value `kessel` now returns
 * as the official name of the v2 stack itself — the v3 + v5
 * migrations compose so a pre-0.39 `kessel` blob still lands on the
 * daemon-hosted stack.)
 */
export type TerminalRenderer = 'alacritty' | 'alacritty-v2' | 'kessel'

/**
 * How the v2 terminal paints its grid (the CellRun rows inside
 * TerminalPane). Orthogonal to `renderer` (which selects the PTY
 * engine/wire): both painters consume the same daemon snapshots.
 *
 * - `dom` (default): the memoized `<span>` row strip. The proven
 *   path — stays byte-identical regardless of this flag's existence.
 * - `webgl`: experimental WebGL2 instanced painter
 *   (`kessel-term/webgl/`). Falls back to `dom` per-pane on context
 *   loss / missing WebGL2. Design: `.k2/notes/webgl-painter-brief.md`.
 *
 * Like `renderer`, the value is read at pane mount — changing it only
 * affects NEW terminal panes.
 */
export type TerminalPainterKind = 'dom' | 'webgl'

/** Cell line-height as a multiple of font size (matches prior Kessel
 *  default of 1.2). Global across styles — Terminal settings, not Styles. */
export const LINE_HEIGHT_MULT_DEFAULT = 1.2
export const LINE_HEIGHT_MULT_MIN = 1.0
export const LINE_HEIGHT_MULT_MAX = 1.6

/** Character tracking: multiplies measured monospace cell width.
 *  1.0 = measured advance; >1 opens spacing (DOM often reads a hair
 *  wider than device-floored WebGL). Global across styles. */
export const CHAR_TRACKING_DEFAULT = 1.0
export const CHAR_TRACKING_MIN = 0.9
export const CHAR_TRACKING_MAX = 1.4

export function clampLineHeightMult(v: number): number {
  if (!Number.isFinite(v)) return LINE_HEIGHT_MULT_DEFAULT
  return Math.min(LINE_HEIGHT_MULT_MAX, Math.max(LINE_HEIGHT_MULT_MIN, v))
}

export function clampCharTracking(v: number): number {
  if (!Number.isFinite(v)) return CHAR_TRACKING_DEFAULT
  return Math.min(CHAR_TRACKING_MAX, Math.max(CHAR_TRACKING_MIN, v))
}

interface TerminalSettingsState {
  fontSize: number
  linkClickMode: LinkClickMode
  openLinksInSplitPane: boolean
  shortcutLayout: ShortcutModifierLayout
  renderer: TerminalRenderer
  painter: TerminalPainterKind
  /**
   * WebGL coverage-gamma for glyph edges (legacy channel; live gamma
   * is owned by the style store — Settings → Styles). Kept for
   * migrate compatibility.
   */
  textGamma: number
  /** Line height multiplier of font size → cell height. WebGL only. */
  lineHeightMultiplier: number
  /** Cell-width multiplier (character tracking). WebGL only. */
  charTracking: number
  incrementFontSize: () => void
  decrementFontSize: () => void
  resetFontSize: () => void
  setLinkClickMode: (mode: LinkClickMode) => void
  setOpenLinksInSplitPane: (enabled: boolean) => void
  setShortcutLayout: (layout: ShortcutModifierLayout) => void
  setRenderer: (renderer: TerminalRenderer) => void
  setPainter: (painter: TerminalPainterKind) => void
  setTextGamma: (v: number) => void
  setLineHeightMultiplier: (v: number) => void
  setCharTracking: (v: number) => void
}

/** Persist migrate for k2so-terminal-settings. Exported for unit tests. */
export function migrateTerminalSettings(
  persisted: unknown,
  version: number,
): Partial<TerminalSettingsState> {
  if (persisted && typeof persisted === 'object') {
    let ps = persisted as {
      renderer?: string
      painter?: string
      textGamma?: number
      lineHeightMultiplier?: number
      charTracking?: number
    }
    if (version < 2 && ps.renderer === 'alacritty') {
      ps = { ...ps, renderer: 'alacritty-v2' }
    }
    if (version < 3 && ps.renderer === 'kessel') {
      ps = { ...ps, renderer: 'alacritty-v2' }
    }
    if (version < 4 && ps.painter === undefined) {
      ps = { ...ps, painter: 'dom' }
    }
    if (version < 5 && (ps.renderer === 'alacritty' || ps.renderer === 'alacritty-v2')) {
      ps = { ...ps, renderer: 'kessel' }
    }
    if (version < 6 && (ps.textGamma === undefined || !Number.isFinite(ps.textGamma))) {
      ps = { ...ps, textGamma: TEXT_GAMMA_LIGHT }
    }
    if (
      version < 7 &&
      (ps.lineHeightMultiplier === undefined || !Number.isFinite(ps.lineHeightMultiplier))
    ) {
      ps = { ...ps, lineHeightMultiplier: LINE_HEIGHT_MULT_DEFAULT }
    }
    if (version < 7 && (ps.charTracking === undefined || !Number.isFinite(ps.charTracking))) {
      ps = { ...ps, charTracking: CHAR_TRACKING_DEFAULT }
    }
    return ps as Partial<TerminalSettingsState>
  }
  return persisted as Partial<TerminalSettingsState>
}

// Persisted via zustand's persist middleware so the user's
// renderer + other preferences survive reload/restart. Prior to
// persistence, toggling to Kessel was silently lost on the next
// app launch — users would swap to Kessel, restart, see Alacritty,
// and assume the setting hadn't taken. Persisted in localStorage
// under the key below.
export const useTerminalSettingsStore = create<TerminalSettingsState>()(
  persist(
    (set) => ({
      fontSize: TERMINAL_FONT_SIZE_DEFAULT,
      linkClickMode: 'click' as LinkClickMode,
      openLinksInSplitPane: true,
      shortcutLayout: 'cmd-active-cmdshift-pinned' as ShortcutModifierLayout,
      // 0.36.7+: default to the daemon-hosted Kessel renderer (it
      // survives Tauri quit and supports heartbeats). Existing
      // users keep their persisted choice via zustand's persist
      // middleware — only fresh installs land on it by default.
      renderer: 'kessel' as TerminalRenderer,
      // WebGL painter is opt-in (experimental); `dom` is the proven
      // default and the permanent fallback path.
      painter: 'dom' as TerminalPainterKind,
      // Light-theme preset; dark styles overwrite via applyStyle.
      // Matches the previous hard-coded WebGL default (c2e634c 1.2).
      textGamma: TEXT_GAMMA_LIGHT,
      lineHeightMultiplier: LINE_HEIGHT_MULT_DEFAULT,
      charTracking: CHAR_TRACKING_DEFAULT,

      incrementFontSize: () => {
        set((state) => ({
          fontSize: Math.min(state.fontSize + 1, TERMINAL_FONT_SIZE_MAX)
        }))
      },

      decrementFontSize: () => {
        set((state) => ({
          fontSize: Math.max(state.fontSize - 1, TERMINAL_FONT_SIZE_MIN)
        }))
      },

      resetFontSize: () => {
        set({ fontSize: TERMINAL_FONT_SIZE_DEFAULT })
      },

      setLinkClickMode: (mode: LinkClickMode) => {
        set({ linkClickMode: mode })
      },

      setOpenLinksInSplitPane: (enabled: boolean) => {
        set({ openLinksInSplitPane: enabled })
      },

      setShortcutLayout: (layout: ShortcutModifierLayout) => {
        set({ shortcutLayout: layout })
      },

      setRenderer: (renderer: TerminalRenderer) => {
        // 0.37.0: 'alacritty' (legacy) is no longer a user-selectable
        // option. The Settings UI hides it from the dropdown; this
        // setter coerces any programmatic attempt to set it (e.g.,
        // someone editing localStorage by hand or invoking via
        // DevTools) so the chosen renderer stays on a supported path.
        // 0.40.x Kessel rename: 'kessel' is the canonical value for
        // the daemon-hosted stack; treat anything else ('alacritty',
        // the pre-rename 'alacritty-v2', unknown future values) as a
        // legacy/unknown value and snap it back.
        const normalized = renderer === 'kessel' ? renderer : 'kessel'
        set({ renderer: normalized })
      },

      setPainter: (painter: TerminalPainterKind) => {
        // Same defensive normalization as setRenderer: any unknown
        // value (hand-edited localStorage, stale future flag) snaps
        // back to the safe default.
        set({ painter: painter === 'webgl' ? 'webgl' : 'dom' })
      },

      setTextGamma: (v: number) => {
        set({ textGamma: clampTextGamma(v) })
      },

      setLineHeightMultiplier: (v: number) => {
        set({ lineHeightMultiplier: clampLineHeightMult(v) })
      },

      setCharTracking: (v: number) => {
        set({ charTracking: clampCharTracking(v) })
      },
    }),
    {
      name: 'k2so-terminal-settings',
      storage: createJSONStorage(() => localStorage),
      // Persist only user-facing settings; never serialize the action
      // closures (they rebuild on load anyway).
      partialize: (state) => ({
        fontSize: state.fontSize,
        linkClickMode: state.linkClickMode,
        openLinksInSplitPane: state.openLinksInSplitPane,
        shortcutLayout: state.shortcutLayout,
        renderer: state.renderer,
        painter: state.painter,
        textGamma: state.textGamma,
        lineHeightMultiplier: state.lineHeightMultiplier,
        charTracking: state.charTracking,
      }),
      version: 7,
      // 0.37.0 (v1 → v2): force-migrate users who had the persisted
      // renderer set to 'alacritty' (Legacy) onto 'alacritty-v2'.
      // The legacy option is removed from the Settings UI and the
      // Rust spawn path is slated for deletion in a later release;
      // this migration ensures no user is left on a renderer that
      // will eventually stop working.
      //
      // 0.39.0 (v2 → v3): the experimental 'kessel' JSON-stream
      // renderer was retired alongside the open-core thin-client
      // cleanup. Any persisted 'kessel' value is migrated forward
      // to 'alacritty-v2' so users land on the only remaining
      // supported renderer on next launch.
      // 0.40.x (v3 → v4): the `painter` field was added (WebGL2
      // instanced painter, experimental, default 'dom'). Pre-v4
      // persisted blobs lack the key; stamp the default explicitly so
      // every stored shape is self-describing.
      // 0.40.x (v4 → v5): the Kessel rename. The v2 stack's official
      // name is Kessel; 'kessel' is the canonical persisted value.
      // Coerce BOTH legacy values ('alacritty' and the pre-rename
      // 'alacritty-v2') forward. The steps compose: a pre-v3 'kessel'
      // (JSON-stream beta) blob flows v3 → 'alacritty-v2' → v5 →
      // 'kessel', landing on the daemon-hosted stack either way.
      // 0.40.46 (v5 → v6): WebGL text-weight gamma. Pre-v6 blobs lack
      // the key; stamp the light-theme default (1.2). Style selection
      // overwrites with the dark (0.7) / light (1.2) preset on the
      // next explicit style change.
      // 0.40.47 (v6 → v7): cell line-height multiplier + character
      // tracking (global Terminal knobs for WebGL/DOM spacing parity).
      migrate: migrateTerminalSettings,
    },
  ),
)

// ── Wire up Tauri event listeners for zoom ──────────────────────────

async function initTerminalZoomListeners(): Promise<void> {
  try {
    const { listen } = await import('@tauri-apps/api/event')

    // GH#639: `await` each `listen()` so the promise it returns (which
    // REJECTS in the headless test env — no Tauri window) funnels into
    // this function's returned promise instead of escaping as an
    // unhandled rejection that flips vitest's exit code. Same fix as
    // `initWorkspaceOpsListeners` in tabs.ts; this module is pulled in
    // transitively by tabs.test.ts, so its rejections surface there too.
    await listen('terminal:zoom-in', () => {
      useTerminalSettingsStore.getState().incrementFontSize()
    })

    await listen('terminal:zoom-out', () => {
      useTerminalSettingsStore.getState().decrementFontSize()
    })

    await listen('terminal:zoom-reset', () => {
      useTerminalSettingsStore.getState().resetFontSize()
    })
  } catch {
    // Tauri API not available (e.g. in tests)
  }
}

// Initialize listeners on import. GH#639: swallow the rejection the
// awaited `listen()` calls produce in the headless test env so it never
// escapes as an unhandled rejection (which flips vitest's exit code).
void initTerminalZoomListeners().catch(() => {})
