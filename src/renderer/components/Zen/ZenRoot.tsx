// prd-zen-mode-v1 Z4, Z6, Z13, Z20, Z23, Z25, Z29 — the Zen layer.
//
// Fills the window (`position:fixed; inset:0; z-index:150`): above App's
// shell, below Settings (200, which hides Zen anyway), the 400 dropdown
// floor, Toast / Command Palette (9999) and dialogs (99998+). K2 renders no
// top bar, PageTabs, sidebar or bottom bar while it is up.
//
// It reads the page for the selected Home from THIS computer's daemon, keeps
// it live on `zen_changed`, applies the Zen theme to its own root only
// (never `<html>`), publishes the stoplight / window-control safe area, and
// drops into safe mode on its own when the page can't be read, crashes, or
// fails the required-controls check.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { useHomesStore, selectedHome } from '@/stores/homes'
import { loadZenPage, useZenConfigStore, watchLocalZenChanged } from '@/lib/zen/zen-api'
import { BUILTIN_TEXTING_PAGE, type ZenResolvedPage, type ZenThemeScope } from '@/lib/zen/zen-page'
import { exitZen, useZenViewStore } from '@/lib/zen/zen-view'
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
import { ZenShortcutSheet, ZenThemePicker } from './ZenThemeTools'
import type { DesktopOs } from '@/lib/desktop-chrome'
import { setZenReservedRects } from '@/lib/zen/zen-monitor'
import { currentDesktopOs } from '@/lib/zen/zen-platform'
import type { ZenControlCheck, ZenRect } from '@/lib/zen/zen-controls'
import { ZenErrorBoundary } from './ZenErrorBoundary'
import { ZenConfigErrorBanner, ZenSafeBanner } from './ZenBanners'
import { ZenChromeCluster, zenClusterSide } from './ZenChromeCluster'
import { ZenPage } from './ZenPage'

// S5: the Zen theme engine (tokens, scheme, type, shape, motion) through the
// S4 plug-in point. Once per app session.
installZenThemeEngine()

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
  const run = useCallback((what: string, send: (scope: ZenThemeScope, homeId: string) => Promise<void>) => {
    const p = pageRef.current
    if (safeRef.current || !p) return
    setError(null)
    const h = selectedHome(useHomesStore.getState())
    void send(p.themeScope, h.id)
      .then(() => {
        useZenOverlayStore.setState({ picker: false })
        const now = selectedHome(useHomesStore.getState())
        return loadZenPage(now.id, now.name)
      })
      .catch((err: unknown) => {
        setError(`Couldn't switch ${what}: ${err instanceof Error ? err.message : String(err)}`)
        useZenOverlayStore.setState({ picker: true })
      })
  }, [])

  const pick = useCallback(
    (name: string) => run(`to ${name}`, (scope, homeId) => setZenTheme(name, scope, homeId)),
    [run],
  )

  useEffect(() => {
    const uninstallKeys = installZenThemeKeys(os, window, {
      cycle: (dir) =>
        run(dir === 1 ? 'to the next theme' : 'to the previous theme', (scope, homeId) =>
          cycleZenTheme(dir, scope, homeId),
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
  const home = useHomesStore((s) => selectedHome(s))
  const config = useZenConfigStore()
  const systemScheme = useSystemScheme()
  const reducedMotion = useMediaFlag(REDUCED_MOTION_QUERY)
  const reducedTransparency = useMediaFlag(REDUCED_TRANSPARENCY_QUERY)
  const os = useMemo(() => currentDesktopOs(), [])
  const inSafeMode = safe !== null
  const livePage = config.homeId === home.id ? config.page : null
  const chrome = useZenChromeHold(livePage?.chrome ?? null, inSafeMode)
  const { area, onClusterRect } = useStoplightArea(chrome)

  // Read the page for this Home (not in safe mode: safe mode never reads the
  // user's files). A failure is safe mode with its cause.
  useEffect(() => {
    if (inSafeMode) return
    let live = true
    void loadZenPage(home.id, home.name).then((result) => {
      if (!live || result.status !== 'failed') return
      enterSafeMode({ kind: result.failure.kind, message: result.failure.message })
    })
    return () => {
      live = false
    }
  }, [home.id, home.name, epoch, inSafeMode, enterSafeMode])

  // `zen_changed` from this computer's daemon: re-read; it also ends safe
  // mode (Z29).
  useEffect(
    () =>
      watchLocalZenChanged(() => {
        const st = useZenViewStore.getState()
        if (st.safe) st.clearSafeMode()
        else {
          const h = selectedHome(useHomesStore.getState())
          void loadZenPage(h.id, h.name).then((result) => {
            if (result.status === 'failed') {
              useZenViewStore.getState().enterSafeMode({ kind: result.failure.kind, message: result.failure.message })
            }
          })
        }
      }),
    [],
  )

  // The control check must stay clear of the stoplights / K2's cluster.
  const reservedRect = area.rect
  useEffect(() => setZenReservedRects(() => [reservedRect]), [reservedRect])

  const page: ZenResolvedPage | null = inSafeMode
    ? BUILTIN_TEXTING_PAGE
    : config.homeId === home.id && config.page
      ? config.page
      : null

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
      enterSafeMode({ kind: 'control', control: f.control, problem: f.problem }),
    [enterSafeMode],
  )
  const onCrash = useCallback((message: string) => enterSafeMode({ kind: 'crash', message }), [enterSafeMode])

  const banner = safe ? (
    <ZenSafeBanner cause={safe} onTryAgain={clearSafeMode} onExit={exitZen} />
  ) : page && page.errors.length > 0 ? (
    <ZenConfigErrorBanner issue={page.errors[0]} />
  ) : null

  return (
    <div
      data-zen-root=""
      data-zen-scheme={theme.scheme}
      data-zen-safe={inSafeMode ? '' : undefined}
      data-zen-reduced-motion={reducedMotion ? '' : undefined}
      data-zen-corners={chrome.corners}
      data-zen-stoplights={chrome.stoplights}
      style={style}
    >
      <style data-zen-keyframes="">{ZEN_KEYFRAMES_CSS}</style>
      <style data-zen-shield="">{ZEN_SHIELD_CSS}</style>
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
      <div style={{ position: 'relative', zIndex: 1, display: 'flex', flexDirection: 'column', flex: 1, minHeight: 0 }}>
      {page ? (
        <ZenErrorBoundary
          key={`${epoch}:${inSafeMode ? 'safe' : 'page'}`}
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
      {page && !inSafeMode && (
        <ZenThemePicker
          themes={page.themes}
          active={page.activeTheme}
          onPick={themeTools.pick}
          error={themeTools.error}
        />
      )}
      <ZenShortcutSheet os={os} />
    </div>
  )
}
