// prd-zen-mode-v1 Z4, Z6, Z13, Z20, Z23, Z25, Z29 and prd-zen-gardens-v1
// G4, G12, G22, G23, G50 — the Zen layer.
//
// Fills the window (`position:fixed; inset:0; z-index:150`): above App's
// shell, below Settings (200, which hides Zen anyway), the 400 dropdown
// floor, Toast / Command Palette (9999) and dialogs (99998+). K2 renders no
// top bar, PageTabs, sidebar or bottom bar while it is up.
//
// It reads the Garden list and the page of this window's Garden from THIS
// computer's daemon, keeps both live on `zen_changed`, applies the Zen
// theme to its own root only (never `<html>`), publishes the stoplight /
// window-control safe area, and drops into safe mode on its own when the
// page can't be read, crashes, or fails the required-controls check. It
// takes keyboard focus when it mounts, so no terminal hidden under it keeps
// typing (G50).

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { loadZenPage, useZenConfigStore, watchLocalZenChanged, type ZenLoadFailure } from '@/lib/zen/zen-api'
import { currentZenGardenId, loadZenGardens, useCurrentZenGarden } from '@/lib/zen/zen-gardens'
import { BUILTIN_TEXTING_PAGE, type ZenResolvedPage, type ZenThemeScope } from '@/lib/zen/zen-page'
import { exitZen, useZenViewStore, type ZenSafeCause } from '@/lib/zen/zen-view'
import {
  REDUCED_MOTION_QUERY,
  REDUCED_TRANSPARENCY_QUERY,
  systemZenScheme,
  zenMediaMatches,
  zenThemeFor,
  type ZenScheme,
} from '@/lib/zen/zen-theme'
import { installZenThemeEngine } from '@/lib/zen/zen-theme-engine'
import { ZEN_KEYFRAMES_CSS } from '@/lib/zen/zen-motion'
import { ZEN_SHIELD_CSS, ZEN_STYLE_SHIELD } from '@/lib/zen/zen-style-shield'
import { ZEN_GLASS_CSS } from '@/lib/zen/zen-glass'
import { noteZenEntered } from '@/lib/zen/zen-compose-focus'
import type { ZenBackgroundFit } from '@/lib/zen/zen-tokens'
import {
  clusterArea,
  currentMacStoplightArea,
  parseZenChrome,
  ZEN_DEFAULT_CHROME,
  type ZenStoplightArea,
} from '@/lib/zen/zen-chrome'
import { onChromeApplied, setChromeSource, type ZenChromeSource } from '@/stores/style'
import { useZenAppliedThemeStore } from '@/lib/zen/zen-theme'
import {
  closeZenOverlays,
  cycleZenTheme,
  installZenThemeKeys,
  openZenCheatSheet,
  setZenTheme,
  useZenOverlayStore,
  ZEN_SHORTCUTS_MENU_EVENT,
} from '@/lib/zen/zen-theme-switch'
import { ZenShortcutSheet } from './ZenThemeTools'
import { ZenAddAgentPicker } from './ZenAddAgent'
import type { DesktopOs } from '@/lib/desktop-chrome'
import { setZenReservedRects } from '@/lib/zen/zen-monitor'
import { currentDesktopOs } from '@/lib/zen/zen-platform'
import type { ZenControlCheck, ZenRect } from '@/lib/zen/zen-controls'
import { ZenErrorBoundary } from './ZenErrorBoundary'
import { ZenConfigErrorBanner, ZenSafeBanner } from './ZenBanners'
import { ZenChromeCluster, zenClusterSide } from './ZenChromeCluster'
import { ZenPage } from './ZenPage'
import { ZenK2ChromeContext, type ZenK2Chrome } from './ZenTemplateControls'

// S5: the Zen theme engine (tokens, scheme, type, shape, motion) through the
// S4 plug-in point. Once per app session.
installZenThemeEngine()

/** A failed read as a safe-mode cause (G19: an older daemon says so). */
function safeCauseFor(failure: ZenLoadFailure): ZenSafeCause {
  return failure.kind === 'outdated' ? { kind: 'outdated' } : { kind: failure.kind, message: failure.message }
}

/** Read the page of this window's Garden; a Garden that went away in the
 *  meantime re-reads the list (which moves the window to the first one). */
async function loadCurrentPage(): Promise<void> {
  const id = currentZenGardenId()
  if (!id) return
  const result = await loadZenPage(id)
  if (result.status !== 'failed') return
  if (/unknown_garden/.test(result.failure.message)) {
    const list = await loadZenGardens()
    if (list.status === 'failed' && list.failure) useZenViewStore.getState().enterSafeMode(safeCauseFor(list.failure))
    return
  }
  if (currentZenGardenId() === id) useZenViewStore.getState().enterSafeMode(safeCauseFor(result.failure))
}

/** `[background] fit` as CSS (the daemon's `cover|contain|tile|center`). */
const ZEN_BACKGROUND_FIT_CSS: Record<ZenBackgroundFit, React.CSSProperties> = {
  cover: { backgroundSize: 'cover', backgroundPosition: 'center', backgroundRepeat: 'no-repeat' },
  contain: { backgroundSize: 'contain', backgroundPosition: 'center', backgroundRepeat: 'no-repeat' },
  tile: { backgroundSize: 'auto', backgroundPosition: '0 0', backgroundRepeat: 'repeat' },
  center: { backgroundSize: 'auto', backgroundPosition: 'center', backgroundRepeat: 'no-repeat' },
}

/** A live `matchMedia` flag (reduced motion / reduced transparency). */
function useMediaFlag(query: string): boolean {
  const [on, setOn] = useState(() => zenMediaMatches(query))
  useEffect(() => {
    if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return
    const mq = window.matchMedia(query)
    const sync = (): void => setOn(mq.matches)
    sync()
    mq.addEventListener?.('change', sync)
    return () => mq.removeEventListener?.('change', sync)
  }, [query])
  return on
}

/**
 * Z23, Z49: the window's native chrome (corners, stoplight shape and
 * offset) comes from the Zen `[chrome]` table while this window shows Zen,
 * through the one chrome owner in `stores/style.ts`. Every Styles re-apply
 * trigger (resize, fullscreen, zoom, title change, a Style hover preview)
 * reads that owner, so Zen's values hold until Zen unmounts; then the
 * Style's values come back. A bad value keeps the last good one.
 */
function useZenChromeHold(raw: unknown, safe: boolean): ZenChromeSource {
  const lastGood = useRef<ZenChromeSource>(ZEN_DEFAULT_CHROME)
  const heldRef = useRef<ZenChromeSource | null>(null)
  const chrome = safe ? ZEN_DEFAULT_CHROME : parseZenChrome(raw, lastGood.current)
  if (!safe) lastGood.current = chrome
  const prev = heldRef.current
  if (
    !prev ||
    prev.corners !== chrome.corners ||
    prev.stoplights !== chrome.stoplights ||
    prev.offset[0] !== chrome.offset[0] ||
    prev.offset[1] !== chrome.offset[1]
  ) {
    heldRef.current = chrome
  }
  const held = heldRef.current ?? chrome
  // Hold: one call per change of value (never a style→zen flip in between).
  useEffect(() => {
    setChromeSource({ zen: held })
  }, [held])
  // Restore the Style's chrome when Zen leaves this window.
  useEffect(() => () => setChromeSource('style'), [])
  return held
}

/**
 * Omarchy additions 2 and 4: switch the daemon's active theme (picker and
 * ⌃⌘. / ⌃⌘⇧.), and open the shortcut cheat sheet (`?`, ⌃⌘/, the menus).
 * Lives only while Zen is shown. Safe mode never changes the user's theme.
 */
function useZenThemeControls(
  os: DesktopOs,
  page: ZenResolvedPage | null,
  safe: boolean,
): { pick(name: string): void; error: string | null } {
  const [error, setError] = useState<string | null>(null)
  const pageRef = useRef(page)
  pageRef.current = page
  const safeRef = useRef(safe)
  safeRef.current = safe

  /** Run one switch on this computer's daemon, then re-read the page. */
  const run = useCallback((what: string, send: (scope: ZenThemeScope, gardenId: string) => Promise<void>) => {
    const p = pageRef.current
    const gardenId = currentZenGardenId()
    if (safeRef.current || !p || !gardenId) return
    setError(null)
    void send(p.themeScope, gardenId)
      .then(() => {
        useZenOverlayStore.setState({ picker: false })
        return loadCurrentPage()
      })
      .catch((err: unknown) => {
        setError(`Couldn't switch ${what}: ${err instanceof Error ? err.message : String(err)}`)
        useZenOverlayStore.setState({ picker: true })
      })
  }, [])

  const pick = useCallback(
    (name: string) => run(`to ${name}`, (scope, gardenId) => setZenTheme(name, scope, gardenId)),
    [run],
  )

  useEffect(() => {
    const uninstallKeys = installZenThemeKeys(os, window, {
      cycle: (dir) =>
        run(dir === 1 ? 'to the next theme' : 'to the previous theme', (scope, gardenId) =>
          cycleZenTheme(dir, scope, gardenId),
        ),
      sheet: openZenCheatSheet,
    })
    // Linux / Windows app menu (DOM event, this window only).
    window.addEventListener(ZEN_SHORTCUTS_MENU_EVENT, openZenCheatSheet)
    // macOS View menu "Zen Shortcuts": emitted to the focused window.
    let unlisten: (() => void) | null = null
    let dead = false
    let label: string | null = null
    try {
      label = getCurrentWindow().label
    } catch {
      label = null
    }
    if (label) {
      void import('@tauri-apps/api/event')
        .then(({ listen }) =>
          listen(ZEN_SHORTCUTS_MENU_EVENT, openZenCheatSheet, { target: { kind: 'AnyLabel', label: label as string } }),
        )
        .then((fn) => {
          if (dead) fn()
          else unlisten = fn
        })
        .catch((err: unknown) => console.warn('[zen] shortcuts menu listen failed:', err))
    }
    return () => {
      dead = true
      uninstallKeys()
      window.removeEventListener(ZEN_SHORTCUTS_MENU_EVENT, openZenCheatSheet)
      unlisten?.()
      closeZenOverlays()
    }
  }, [os, run])

  return { pick, error }
}

function useSystemScheme(): ZenScheme {
  const [scheme, setScheme] = useState<ZenScheme>(() => systemZenScheme())
  useEffect(() => {
    if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return
    const mq = window.matchMedia('(prefers-color-scheme: dark)')
    const on = (): void => setScheme(mq.matches ? 'dark' : 'light')
    mq.addEventListener?.('change', on)
    return () => mq.removeEventListener?.('change', on)
  }, [])
  return scheme
}

/** The safe area the window's own buttons take (Z23, Z25). */
function useStoplightArea(chrome: ZenChromeSource): {
  area: ZenStoplightArea
  onClusterRect(rect: ZenRect | null): void
} {
  const os = useMemo(() => currentDesktopOs(), [])
  const side = zenClusterSide(os)
  const [cluster, setCluster] = useState<ZenRect | null>(null)
  const [, setTick] = useState(0)
  // macOS: the lights move with the app zoom (every apply, zoom included,
  // notifies) and with a resize.
  useEffect(() => {
    if (os !== 'mac') return
    const bump = (): void => setTick((n) => n + 1)
    window.addEventListener('resize', bump)
    const off = onChromeApplied(bump)
    return () => {
      window.removeEventListener('resize', bump)
      off()
    }
  }, [os])
  const area =
    os === 'mac'
      ? currentMacStoplightArea(chrome)
      : side
        ? clusterArea(cluster, side, typeof window === 'undefined' ? 0 : window.innerWidth)
        : clusterArea(null, 'left', 0)
  const onClusterRect = useCallback((rect: ZenRect | null) => {
    setCluster((prev) =>
      prev && rect && prev.left === rect.left && prev.top === rect.top && prev.width === rect.width && prev.height === rect.height
        ? prev
        : rect,
    )
  }, [])
  return { area, onClusterRect }
}

export function ZenRoot(): React.JSX.Element {
  const safe = useZenViewStore((s) => s.safe)
  const epoch = useZenViewStore((s) => s.epoch)
  const enterSafeMode = useZenViewStore((s) => s.enterSafeMode)
  const clearSafeMode = useZenViewStore((s) => s.clearSafeMode)
  const garden = useCurrentZenGarden()
  const gardenId = garden?.id ?? null
  const config = useZenConfigStore()
  const rootRef = useRef<HTMLDivElement | null>(null)
  const systemScheme = useSystemScheme()
  const reducedMotion = useMediaFlag(REDUCED_MOTION_QUERY)
  const reducedTransparency = useMediaFlag(REDUCED_TRANSPARENCY_QUERY)
  const os = useMemo(() => currentDesktopOs(), [])
  const inSafeMode = safe !== null
  const livePage = gardenId !== null && config.gardenId === gardenId ? config.page : null
  const chrome = useZenChromeHold(livePage?.chrome ?? null, inSafeMode)
  const { area, onClusterRect } = useStoplightArea(chrome)

  // Entering Zen: the page's first view may put the caret in its selected
  // agent's message box (`zen-compose-focus`). Marked during this render,
  // before any child effect (the page may already be cached and mount now).
  useState(noteZenEntered)

  // G50: keys typed after the toggle land in Zen, never in a terminal
  // hidden underneath.
  useEffect(() => {
    const el = rootRef.current
    if (!el || (document.activeElement && el.contains(document.activeElement))) return
    el.focus({ preventScroll: true })
  }, [])

  // Read the Garden list (not in safe mode: safe mode never reads the
  // user's files). Zen on with no folder yet sets it up once (G22). A
  // failure is safe mode with its cause.
  useEffect(() => {
    if (inSafeMode) return
    let live = true
    void loadZenGardens().then((result) => {
      if (live && result.status === 'failed' && result.failure) enterSafeMode(safeCauseFor(result.failure))
    })
    return () => {
      live = false
    }
  }, [epoch, inSafeMode, enterSafeMode])

  // Read the page of this window's Garden, again on a Garden switch.
  useEffect(() => {
    if (inSafeMode || gardenId === null) return
    void loadCurrentPage()
  }, [gardenId, epoch, inSafeMode])

  // `zen_changed` from this computer's daemon: re-read the list and the
  // page (G15); it also ends safe mode (Z29).
  useEffect(
    () =>
      watchLocalZenChanged(() => {
        const st = useZenViewStore.getState()
        if (st.safe) st.clearSafeMode()
        else {
          void loadZenGardens().then((result) => {
            if (result.status === 'failed' && result.failure) {
              useZenViewStore.getState().enterSafeMode(safeCauseFor(result.failure))
              return
            }
            return loadCurrentPage()
          })
        }
      }),
    [],
  )

  // The control check must stay clear of the stoplights / K2's cluster.
  const reservedRect = area.rect
  useEffect(() => setZenReservedRects(() => [reservedRect]), [reservedRect])

  const page: ZenResolvedPage | null = inSafeMode ? BUILTIN_TEXTING_PAGE : livePage

  const theme = zenThemeFor(page, systemScheme, inSafeMode, { reducedMotion, reducedTransparency })
  const themeTools = useZenThemeControls(os, page, inSafeMode)

  // Terminals shown in Zen read the bundle's palette and font from here.
  const terminalKey = theme.terminal ? JSON.stringify(theme.terminal) : ''
  const terminalFont = theme.vars['--zen-terminal-font-family'] ?? ''
  useEffect(() => {
    useZenAppliedThemeStore.setState({
      applied: theme.terminal
        ? { name: theme.name ?? null, scheme: theme.scheme, terminal: theme.terminal, terminalFont }
        : null,
    })
  }, [terminalKey, terminalFont, theme.name, theme.scheme])
  useEffect(() => () => useZenAppliedThemeStore.setState({ applied: null }), [])
  const style = {
    position: 'fixed',
    inset: 0,
    zIndex: 150,
    outline: 'none',
    display: 'flex',
    flexDirection: 'column',
    background: 'var(--zen-canvas)',
    color: 'var(--zen-text)',
    fontFamily: 'var(--zen-font-family)',
    fontSize: 'var(--zen-font-size)',
    lineHeight: 'var(--zen-line-height)',
    // Form controls, scrollbars and UA defaults follow Zen's scheme, not the Style's.
    colorScheme: theme.scheme,
    // Reused app components read Styles variables: inside Zen they are Zen tokens.
    ...ZEN_STYLE_SHIELD,
    ...theme.vars,
    ...area.vars,
  } as React.CSSProperties

  const onControlFailure = useCallback(
    (f: Extract<ZenControlCheck, { ok: false }>) =>
      enterSafeMode({ kind: 'control', control: f.control, problem: f.problem, ...(f.menu ? { menu: f.menu } : {}) }),
    [enterSafeMode],
  )
  const onCrash = useCallback((message: string) => enterSafeMode({ kind: 'crash', message }), [enterSafeMode])

  // K2's own extras, one per chrome kind (prd-zen-freeform-chrome FC50):
  // the usage tool and the theme control, wherever the page places them
  // (the template: immediately left of the Zen toggle, Rosson 2026-10-04).
  // None in safe mode: safe mode never changes the theme and reads nothing
  // it doesn't need (FC22).
  const themePick = themeTools.pick
  const themeError = themeTools.error
  const themes = page?.themes
  const activeTheme = page?.activeTheme ?? null
  const k2Chrome = useMemo<ZenK2Chrome>(
    () =>
      themes && !inSafeMode
        ? { usage: true, theme: { themes, active: activeTheme, onPick: themePick, error: themeError } }
        : { usage: false, theme: null },
    [themes, activeTheme, themePick, themeError, inSafeMode],
  )

  const banner = safe ? (
    <ZenSafeBanner cause={safe} onTryAgain={clearSafeMode} onExit={exitZen} />
  ) : page && page.errors.length > 0 ? (
    <ZenConfigErrorBanner issue={page.errors[0]} />
  ) : null

  return (
    <div
      ref={rootRef}
      tabIndex={-1}
      data-zen-root=""
      data-zen-garden={gardenId ?? undefined}
      data-zen-scheme={theme.scheme}
      data-zen-safe={inSafeMode ? '' : undefined}
      data-zen-reduced-motion={reducedMotion ? '' : undefined}
      data-zen-corners={chrome.corners}
      data-zen-stoplights={chrome.stoplights}
      style={style}
    >
      <style data-zen-keyframes="">{ZEN_KEYFRAMES_CSS}</style>
      <style data-zen-shield="">{ZEN_SHIELD_CSS}</style>
      <style data-zen-glass-styles="">{ZEN_GLASS_CSS}</style>
      {theme.background && (
        <div
          aria-hidden
          data-zen-background=""
          data-zen-background-fit={theme.background.fit}
          style={{
            position: 'absolute',
            inset: 0,
            zIndex: 0,
            backgroundImage: `url("${theme.background.src}")`,
            ...ZEN_BACKGROUND_FIT_CSS[theme.background.fit],
            // Over the root's canvas: the canvas shows through the rest.
            opacity: theme.background.opacity,
            pointerEvents: 'none',
          }}
        />
      )}
      <ZenChromeCluster os={os} onRect={onClusterRect} />
      <ZenK2ChromeContext.Provider value={k2Chrome}>
      <div style={{ position: 'relative', zIndex: 1, display: 'flex', flexDirection: 'column', flex: 1, minHeight: 0 }}>
      {page ? (
        <ZenErrorBoundary
          key={`${epoch}:${inSafeMode ? 'safe' : `page:${gardenId ?? ''}`}`}
          safe={inSafeMode}
          onCrash={onCrash}
          onExit={exitZen}
          banner={banner}
        >
          <ZenPage page={page} safe={inSafeMode} banner={banner} onControlFailure={onControlFailure} />
        </ZenErrorBoundary>
      ) : (
        <div className="flex h-full w-full items-center justify-center" data-zen-loading="" style={{ color: 'var(--zen-text-muted)' }}>
          Opening Zen…
        </div>
      )}
      </div>
      </ZenK2ChromeContext.Provider>
      <ZenShortcutSheet os={os} />
      <ZenAddAgentPicker />
    </div>
  )
}
