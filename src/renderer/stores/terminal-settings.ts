import { create } from 'zustand'
import { persist, createJSONStorage } from 'zustand/middleware'
import {
  TERMINAL_FONT_SIZE_MIN,
  TERMINAL_FONT_SIZE_MAX,
  TERMINAL_FONT_SIZE_DEFAULT
} from '../../shared/constants'

export type LinkClickMode = 'click' | 'cmd-click'
export type ShortcutModifierLayout = 'cmd-active-cmdshift-pinned' | 'cmd-pinned-cmdshift-active'

/**
 * Terminal rendering backend selection.
 *
 * - `alacritty-v2` (default since 0.36.7): daemon-hosted PTY +
 *   alacritty_terminal::Term. Tauri is a pure viewer rendering
 *   daemon-pushed grid snapshots + deltas. Session survives Tauri
 *   quit; heartbeats can target it. Labeled "Alacritty" in the UI.
 *   The A1–A5 phase plan from `.k2so/prds/alacritty-v2.md` landed
 *   in 0.34–0.36; v2 is now production-hardened.
 * - `alacritty` (legacy): in-process alacritty_terminal engine + DOM
 *   renderer. PTY lives in the Tauri process; session dies with the
 *   app. Removed from the Settings UI in 0.37.0; the setter and
 *   persist migration both coerce it to `alacritty-v2`. The legacy
 *   Rust spawn path remains compiled in to host any pre-existing
 *   in-flight tabs gracefully and is slated for removal in a later
 *   release.
 *
 * Changes to this setting only affect NEW tabs; already-open tabs
 * keep their chosen renderer. Zustand's persist middleware means
 * existing users keep whatever they had set — only fresh installs
 * see the new `alacritty-v2` default.
 *
 * 0.39.0: The experimental `kessel` JSON-stream renderer was
 * retired; any persisted value gets coerced to `alacritty-v2` on
 * load via the v3 migration below.
 */
export type TerminalRenderer = 'alacritty' | 'alacritty-v2'

interface TerminalSettingsState {
  fontSize: number
  linkClickMode: LinkClickMode
  openLinksInSplitPane: boolean
  shortcutLayout: ShortcutModifierLayout
  renderer: TerminalRenderer
  incrementFontSize: () => void
  decrementFontSize: () => void
  resetFontSize: () => void
  setLinkClickMode: (mode: LinkClickMode) => void
  setOpenLinksInSplitPane: (enabled: boolean) => void
  setShortcutLayout: (layout: ShortcutModifierLayout) => void
  setRenderer: (renderer: TerminalRenderer) => void
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
      // 0.36.7+: default to alacritty-v2 (the daemon-hosted renderer
      // that survives Tauri quit and supports heartbeats). Existing
      // users keep their persisted choice via zustand's persist
      // middleware — only fresh installs land on v2 by default.
      renderer: 'alacritty-v2' as TerminalRenderer,

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
        // 0.39.0: 'kessel' was retired; same coercion applies. Treat
        // anything other than 'alacritty-v2' as a legacy/unknown
        // value and snap it back.
        const normalized = renderer === 'alacritty-v2' ? renderer : 'alacritty-v2'
        set({ renderer: normalized })
      }
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
      }),
      version: 3,
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
      migrate: (persisted: unknown, version: number) => {
        if (persisted && typeof persisted === 'object') {
          const ps = persisted as { renderer?: string }
          if (version < 2 && ps.renderer === 'alacritty') {
            return { ...ps, renderer: 'alacritty-v2' }
          }
          if (version < 3 && ps.renderer === 'kessel') {
            return { ...ps, renderer: 'alacritty-v2' }
          }
        }
        return persisted as Partial<TerminalSettingsState>
      },
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
