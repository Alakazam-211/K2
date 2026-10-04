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

import { useCallback, useEffect, useMemo, useState } from 'react'
import { useHomesStore, selectedHome } from '@/stores/homes'
import { loadZenPage, useZenConfigStore, watchLocalZenChanged } from '@/lib/zen/zen-api'
import { BUILTIN_TEXTING_PAGE, type ZenResolvedPage } from '@/lib/zen/zen-page'
import { exitZen, useZenViewStore } from '@/lib/zen/zen-view'
import { systemZenScheme, zenThemeFor, type ZenScheme } from '@/lib/zen/zen-theme'
import { clusterArea, currentMacStoplightArea, type ZenStoplightArea } from '@/lib/zen/zen-chrome'
import { setZenReservedRects } from '@/lib/zen/zen-monitor'
import { currentDesktopOs } from '@/lib/zen/zen-platform'
import type { ZenControlCheck, ZenRect } from '@/lib/zen/zen-controls'
import { ZenErrorBoundary } from './ZenErrorBoundary'
import { ZenConfigErrorBanner, ZenSafeBanner } from './ZenBanners'
import { ZenChromeCluster, zenClusterSide } from './ZenChromeCluster'
import { ZenPage } from './ZenPage'

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
function useStoplightArea(): { area: ZenStoplightArea; onClusterRect(rect: ZenRect | null): void } {
  const os = useMemo(() => currentDesktopOs(), [])
  const side = zenClusterSide(os)
  const [cluster, setCluster] = useState<ZenRect | null>(null)
  const [, setTick] = useState(0)
  // macOS: the lights move with the app zoom (a resize-free change).
  useEffect(() => {
    if (os !== 'mac') return
    const bump = (): void => setTick((n) => n + 1)
    window.addEventListener('resize', bump)
    return () => window.removeEventListener('resize', bump)
  }, [os])
  const area =
    os === 'mac'
      ? currentMacStoplightArea()
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
  const os = useMemo(() => currentDesktopOs(), [])
  const { area, onClusterRect } = useStoplightArea()

  // Read the page for this Home (not in safe mode: safe mode never reads the
  // user's files). A failure is safe mode with its cause.
  const inSafeMode = safe !== null
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

  const theme = zenThemeFor(page, systemScheme, inSafeMode)
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
    <div data-zen-root="" data-zen-scheme={theme.scheme} data-zen-safe={inSafeMode ? '' : undefined} style={style}>
      <ZenChromeCluster os={os} onRect={onClusterRect} />
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
  )
}
