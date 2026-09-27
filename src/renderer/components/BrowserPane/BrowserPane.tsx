import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { useIsTabVisible } from '@/contexts/TabVisibilityContext'
import { usePageViewStore } from '@/stores/page-view'
import { useSettingsStore } from '@/stores/settings'
import { useTabsStore } from '@/stores/tabs'
import { useContextMenuStore } from '@/stores/context-menu'
import { useWindowFocusStore } from '@/stores/window-focus'
import { useConnectHostStore } from '@/stores/connect-host'
import {
  LOOPBACK_ON_REMOTE_ERROR,
  loopbackForbiddenOnRemote,
} from '@/lib/loopback-remote'
import { webFeatures } from '@/web/features'
import { normalizeUrl } from './normalizeUrl'

export { normalizeUrl }

/** Cmd+L on a visible browser tab. Hidden panes stay mounted, so skip those. */
export function focusVisibleBrowserAddress(): boolean {
  const inputs = document.querySelectorAll<HTMLInputElement>('[data-browser-address]')
  for (const input of inputs) {
    if (input.closest('[aria-hidden="true"]')) continue
    input.focus()
    input.select()
    return true
  }
  return false
}

/**
 * Embedded Browser Tab pane (PRD .k2/prds/prd-browser-pane-v1.md).
 *
 * The browsed page does NOT render in this component — it lives in a
 * NATIVE child webview owned by src-tauri (commands/browser_webviews.rs,
 * label `browser-{parent}-{itemId}`), which floats over the DOM
 * unconditionally, docked in the *invoking* Tauri window (main or
 * window-{uuid}). This component is the DOM-side "docking frame":
 *
 *  - bounds bridge: a ResizeObserver on the content area + a window
 *    resize listener feed a rAF-throttled `browser_set_bounds`, plus a
 *    150ms settle re-assert after window resizes (tauri #10131/#14843
 *    manifest as stale child bounds after resize/restore).
 *  - visibility bridge: `useIsTabVisible()` (tab visible AND this item
 *    active in its pane group) plus Settings / full-page overlays drive
 *    `browser_set_visible` — native child webviews float OVER the DOM, so
 *    hiding the workspace with `display:none` is not enough (Settings
 *    was showing Google sign-in still painted on Companion). Non-standalone
 *    panes also hide when this window loses OS focus so a blurred
 *    window's children do not paint over the focused window.
 *  - lifecycle: the webview is created lazily on FIRST visibility (so a
 *    restored background layout doesn't load pages invisibly) and
 *    `browser_close`d on unmount (item/tab close).
 *
 * Chrome: back and forward for this webview's page history (left of the
 * address field), the address field (Enter → navigate), reload, and
 * devtools (dev builds). Arrows call `browser_back` / `browser_forward`.
 * They do not walk the tab strip. ⌘[ / ⌘] stay on the window.
 */

interface BrowserPaneProps {
  /** Tabs-store item id — the registry key on the Rust side. */
  itemId: string
  tabId: string
  paneGroupId: string
  /** Canonical URL from the tabs store (BrowserItemData.url). */
  url: string
  /**
   * Settings / modal embed: always treat as visible (ignore tab
   * visibility), and do not stamp the tabs store. Still uses the same
   * native child-webview docking as tab panes.
   */
  standalone?: boolean
}

interface Rect {
  x: number
  y: number
  width: number
  height: number
}

/** Message shown when the Rust stub (browser-pane feature off) rejects. */
const STUB_ERROR_FRAGMENT = 'not enabled in this build'

function refuseLoopbackOnRemote(targetUrl: string): string | null {
  const activeHost = useConnectHostStore.getState().activeHost
  if (loopbackForbiddenOnRemote(activeHost, targetUrl)) {
    return LOOPBACK_ON_REMOTE_ERROR
  }
  return null
}

/** Stable parent window label for this renderer instance. */
function currentParentWindow(): string {
  try {
    return getCurrentWindow().label || 'main'
  } catch {
    return 'main'
  }
}

export function BrowserPane({
  itemId,
  tabId,
  paneGroupId,
  url,
  standalone = false,
}: BrowserPaneProps): React.JSX.Element {
  const tabVisible = useIsTabVisible()
  // Settings is a fixed overlay while the workspace stays mounted (display:none
  // only). Projects / Feedback / Wiki are full-page overlays on top of agents.
  // Native WKWebViews ignore that DOM hide — force them off unless this pane
  // is the intentional Settings embed (`standalone`).
  const settingsOpen = useSettingsStore((s) => s.settingsOpen)
  const appPage = usePageViewStore((s) => s.page)
  const workspaceCovered = settingsOpen || appPage !== 'agents'
  // Blurred windows: hide non-standalone children so they don't float over
  // the focused window (multi-window parenting). Standalone OAuth embeds
  // stay visible while their host window is frontmost enough to complete
  // the flow; they still hide when the host itself is covered.
  const windowFocused = useWindowFocusStore((s) => s.isFocused)
  // The + menu and other context menus are DOM. The native page paints
  // over them, so hide the page while a menu is open.
  const menuOpen = useContextMenuStore((s) => s.isOpen)
  const visible = standalone
    ? !menuOpen
    : tabVisible && !workspaceCovered && windowFocused && !menuOpen
  const setBrowserItemState = useTabsStore((s) => s.setBrowserItemState)

  // Parent window for all browser_* invokes (main / window-{uuid}).
  const parentWindow = useMemo(() => currentParentWindow(), [])

  // Content area the native view docks onto (below the chrome bar).
  const contentRef = useRef<HTMLDivElement | null>(null)

  const [created, setCreated] = useState(false)
  const createdRef = useRef(false)
  /** Prevent concurrent `browser_create` (visibility effect + ResizeObserver
   *  both fire before the first await resolves → "webview with label …
   *  already exists" on Email Link Gmail OAuth). */
  const createInFlightRef = useRef(false)
  /** Feature-off stub build / hosted web → render the graceful-degradation message. */
  const [unavailable, setUnavailable] = useState(!webFeatures.browserPane)
  const [error, setError] = useState<string | null>(null)
  // Page history. A new tab, a missing view, the stub, and hosted web
  // start disabled. Failures here must not use the red error strip.
  const [canBack, setCanBack] = useState(false)
  const [canForward, setCanForward] = useState(false)

  // Address bar. Mirrors the polled current URL unless focused (don't
  // clobber the user's in-progress edit).
  const [address, setAddress] = useState(url)
  const addressFocusedRef = useRef(false)
  const addressRef = useRef<HTMLInputElement>(null)
  const skipFocusLockRef = useRef(false)

  /** Last URL WE navigated to or observed via polling — suppresses the
   *  echo loop where the poll stamps the store, the store re-renders us
   *  with a new `url` prop, and the prop effect re-navigates. */
  const lastKnownUrlRef = useRef<string>('')
  /** URL we want loaded once the dock has a non-zero rect (Settings embeds
   *  often measure 0×0 on the first paint, then grow). */
  const pendingUrlRef = useRef<string>('')
  const visibleRef = useRef(visible)
  visibleRef.current = visible

  // ── Bounds bridge ───────────────────────────────────────────────────
  const rafRef = useRef<number | null>(null)
  const settleTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  const measureRect = useCallback((): Rect | null => {
    const el = contentRef.current
    if (!el) return null
    // getBoundingClientRect is CSS px relative to the viewport; the main
    // webview fills the window at origin, so this IS the parent-window
    // logical coordinate space browser_set_bounds expects.
    const r = el.getBoundingClientRect()
    if (r.width <= 0 || r.height <= 0) return null // display:none / collapsed
    return { x: r.x, y: r.y, width: r.width, height: r.height }
  }, [])

  const createView = useCallback(async (targetUrl: string): Promise<void> => {
    // Hosted web: no native child webview — stay on the unavailable stub.
    if (!webFeatures.browserPane) {
      setUnavailable(true)
      return
    }
    const normalized = normalizeUrl(targetUrl)
    if (!normalized) return
    const remoteErr = refuseLoopbackOnRemote(normalized)
    if (remoteErr) {
      pendingUrlRef.current = ''
      setError(remoteErr)
      return
    }
    pendingUrlRef.current = normalized
    if (createdRef.current || createInFlightRef.current) return
    const rect = measureRect()
    if (!rect) {
      // Dock not laid out yet (common in Settings full-pane embeds). ResizeObserver
      // will retry when the content area gets a real size — do NOT clear pending.
      return
    }
    setError(null)
    createInFlightRef.current = true
    try {
      await invoke('browser_create', { itemId, url: normalized, rect, parentWindow })
      createdRef.current = true
      setCreated(true)
      setUnavailable(false)
      lastKnownUrlRef.current = normalized
      pendingUrlRef.current = ''
    } catch (e) {
      const msg = String(e)
      if (msg.includes(STUB_ERROR_FRAGMENT)) {
        setUnavailable(true)
      } else {
        setError(msg)
      }
    } finally {
      createInFlightRef.current = false
    }
  }, [itemId, measureRect, parentWindow])

  const scheduleBoundsPush = useCallback(() => {
    if (rafRef.current !== null) return
    rafRef.current = requestAnimationFrame(() => {
      rafRef.current = null
      // First-paint retry: Settings OAuth dock often starts at 0×0 then
      // expands; createView no-ops until measureRect succeeds.
      if (!createdRef.current && visibleRef.current && pendingUrlRef.current) {
        void createView(pendingUrlRef.current)
        return
      }
      if (!createdRef.current) return
      const rect = measureRect()
      if (!rect) return
      void invoke('browser_set_bounds', { itemId, rect, parentWindow }).catch(() => {
        // View may have been closed in a race; the visibility/lifecycle
        // effects own recovery.
      })
    })
  }, [itemId, measureRect, createView, parentWindow])

  useEffect(() => {
    const el = contentRef.current
    if (!el) return
    const ro = new ResizeObserver(() => scheduleBoundsPush())
    ro.observe(el)
    const onWindowResize = (): void => {
      scheduleBoundsPush()
      // Settle re-assert: child-view bounds can lag the window resize
      // (tauri #10131/#14843) — push once more after the dust settles.
      if (settleTimerRef.current) clearTimeout(settleTimerRef.current)
      settleTimerRef.current = setTimeout(scheduleBoundsPush, 150)
    }
    window.addEventListener('resize', onWindowResize)
    // Double-rAF: wait for flex layout after Settings panel expands.
    requestAnimationFrame(() => scheduleBoundsPush())
    return () => {
      ro.disconnect()
      window.removeEventListener('resize', onWindowResize)
      if (settleTimerRef.current) clearTimeout(settleTimerRef.current)
      if (rafRef.current !== null) {
        cancelAnimationFrame(rafRef.current)
        rafRef.current = null
      }
    }
  }, [scheduleBoundsPush])

  const refreshHistory = useCallback(async (): Promise<void> => {
    if (!webFeatures.browserPane || unavailable || !createdRef.current) {
      setCanBack(false)
      setCanForward(false)
      return
    }
    try {
      const state = await invoke<{ canBack: boolean; canForward: boolean }>(
        'browser_history_state',
        { itemId, parentWindow },
      )
      setCanBack(Boolean(state?.canBack))
      setCanForward(Boolean(state?.canForward))
    } catch {
      // Stub ("not enabled") and a dead child. Not the red strip.
      setCanBack(false)
      setCanForward(false)
    }
  }, [itemId, parentWindow, unavailable])

  const readCurrentUrl = useCallback(async (): Promise<void> => {
    try {
      const current = await invoke<string>('browser_current_url', { itemId, parentWindow })
      if (!current) return
      lastKnownUrlRef.current = current
      if (!addressFocusedRef.current) setAddress(current)
      if (!standalone) {
        setBrowserItemState(tabId, paneGroupId, itemId, { url: current })
      }
    } catch {
      // View gone (close race). The address poll owns recovery.
    }
  }, [itemId, parentWindow, standalone, setBrowserItemState, tabId, paneGroupId])

  const navigateView = useCallback(async (targetUrl: string): Promise<void> => {
    const remoteErr = refuseLoopbackOnRemote(targetUrl)
    if (remoteErr) {
      setError(remoteErr)
      return
    }
    setError(null)
    try {
      await invoke('browser_navigate', { itemId, url: targetUrl, parentWindow })
      lastKnownUrlRef.current = targetUrl
      await refreshHistory()
    } catch (e) {
      setError(String(e))
    }
  }, [itemId, parentWindow, refreshHistory])

  // ── Visibility bridge + lazy creation ───────────────────────────────
  useEffect(() => {
    if (visible) {
      if (!createdRef.current) {
        const target = normalizeUrl(url)
        if (target) void createView(target)
      } else {
        void invoke('browser_set_visible', { itemId, visible: true, parentWindow }).catch(() => {})
        // Bounds may have gone stale while hidden (mosaic resizes under
        // display:none don't reach the native view) — re-assert on show.
        scheduleBoundsPush()
      }
    } else if (createdRef.current) {
      void invoke('browser_set_visible', { itemId, visible: false, parentWindow }).catch(() => {})
    }
  }, [visible, url, itemId, createView, scheduleBoundsPush, parentWindow])

  // ── Store url changes (openUrlInPane navigate-in-place reuse) ───────
  useEffect(() => {
    if (!url) return
    if (!addressFocusedRef.current) setAddress(url)
    if (url === lastKnownUrlRef.current) return
    if (createdRef.current) {
      void navigateView(url)
    } else if (visibleRef.current) {
      void createView(normalizeUrl(url))
    }
    // Hidden + not created: the visibility effect creates on first show.
  }, [url, navigateView, createView])

  // ── Close on unmount ────────────────────────────────────────────────
  // Always attempt close for this itemId (not only when createdRef is
  // true): a create may have completed on the native side while the
  // await was aborted by unmount (Email Link Cancel / success), leaving
  // the label orphaned and the next `browser_create` for the same id
  // (or a remount) hard-colliding.
  useEffect(() => {
    return () => {
      createdRef.current = false
      createInFlightRef.current = false
      void invoke('browser_close', { itemId, parentWindow }).catch(() => {})
    }
  }, [itemId, parentWindow])

  // ── Address-bar sync: poll current URL only while visible ──────────
  // Same 1500ms tick re-reads page history. In-page link clicks never
  // touch the address store until this poll.
  useEffect(() => {
    if (!visible || !created) return
    const timer = setInterval(() => {
      void (async () => {
        try {
          const current = await invoke<string>('browser_current_url', { itemId, parentWindow })
          if (current && current !== lastKnownUrlRef.current) {
            lastKnownUrlRef.current = current
            if (!addressFocusedRef.current) setAddress(current)
            // Stamp the store so serialize captures in-page navigation.
            // Standalone embeds (Settings OAuth) are not tab items.
            if (!standalone) {
              setBrowserItemState(tabId, paneGroupId, itemId, { url: current })
            }
          }
        } catch {
          // View gone (close race) — the interval is cleared by unmount.
        }
        await refreshHistory()
      })()
    }, 1500)
    return () => clearInterval(timer)
  }, [visible, created, itemId, tabId, paneGroupId, setBrowserItemState, standalone, parentWindow, refreshHistory])

  // ── Handlers ────────────────────────────────────────────────────────
  const handleSubmit = useCallback(() => {
    const target = normalizeUrl(address)
    if (!target) return
    setAddress(target)
    // Stamp immediately so the layout autosave captures the intent even
    // if create/navigate fails or the poll hasn't run yet.
    if (!standalone) {
      setBrowserItemState(tabId, paneGroupId, itemId, { url: target })
    }
    if (createdRef.current) {
      void navigateView(target)
    } else {
      void createView(target)
    }
  }, [
    address,
    tabId,
    paneGroupId,
    itemId,
    setBrowserItemState,
    navigateView,
    createView,
    standalone,
  ])

  const handleReload = useCallback(() => {
    const target = lastKnownUrlRef.current || normalizeUrl(address)
    if (!target) return
    if (createdRef.current) {
      void navigateView(target)
    } else {
      void createView(target)
    }
  }, [address, navigateView, createView])

  const stepHistory = useCallback((command: 'browser_back' | 'browser_forward') => {
    if (!webFeatures.browserPane || unavailable || !createdRef.current) return
    void (async () => {
      try {
        await invoke(command, { itemId, parentWindow })
      } catch {
        // No view / stub. Do not setError — that strip is for navigate.
        setCanBack(false)
        setCanForward(false)
        return
      }
      await readCurrentUrl()
      await refreshHistory()
    })()
  }, [itemId, parentWindow, unavailable, readCurrentUrl, refreshHistory])

  // A new browser tab opens with the address selected, ready to type.
  // Programmatic focus must not lock the field, or a later URL poll
  // cannot fill it.
  useEffect(() => {
    const el = addressRef.current
    if (!el) return
    skipFocusLockRef.current = true
    el.focus()
    el.select()
    skipFocusLockRef.current = false
  }, [])

  // ── Render ──────────────────────────────────────────────────────────
  const historyReady = webFeatures.browserPane && !unavailable && created
  const historyButtonClass = (enabled: boolean): string =>
    `flex h-5 w-5 items-center justify-center flex-shrink-0 ${
      enabled
        ? 'text-[var(--color-text-secondary)] hover:text-[var(--color-text)]'
        : 'text-[var(--color-text-muted)] opacity-30'
    }`

  // Placeholder only renders when no native view covers the dock area;
  // once created, the child webview floats over the DOM, so errors that
  // happen mid-session surface in the strip under the chrome bar instead.
  let placeholder: React.ReactNode = null
  if (unavailable) {
    placeholder = webFeatures.browserPane
      ? 'Browser pane not available in this build'
      : 'Embedded browser is not available in the web client'
  } else if (!created) {
    placeholder = error ?? 'Enter a URL to browse'
  }

  return (
    <div className="flex h-full w-full flex-col">
      {/* Chrome bar — styled after the FileViewerPane header. */}
      <div className="flex h-9 items-center gap-2 border-b border-[var(--color-border)] bg-[var(--color-bg)] pl-3 flex-shrink-0">
        <div className="flex items-center gap-0.5 flex-shrink-0">
        <button
          type="button"
          className={historyButtonClass(historyReady && canBack)}
          disabled={!historyReady || !canBack}
          onClick={() => stepHistory('browser_back')}
          title="Back"
          aria-label="Back"
        >
          <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
            <polyline points="6 2 3 5 6 8" />
          </svg>
        </button>
        <button
          type="button"
          className={historyButtonClass(historyReady && canForward)}
          disabled={!historyReady || !canForward}
          onClick={() => stepHistory('browser_forward')}
          title="Forward"
          aria-label="Forward"
        >
          <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
            <polyline points="4 2 7 5 4 8" />
          </svg>
        </button>
        </div>
        <input
          ref={addressRef}
          data-browser-address=""
          type="text"
          value={address}
          spellCheck={false}
          autoCorrect="off"
          autoCapitalize="off"
          placeholder="Enter URL…"
          onChange={(e) => setAddress(e.target.value)}
          onFocus={(e) => {
            if (!skipFocusLockRef.current) addressFocusedRef.current = true
            e.target.select()
          }}
          onBlur={() => {
            addressFocusedRef.current = false
            // Snap back to the real URL if the edit was abandoned.
            if (lastKnownUrlRef.current) setAddress(lastKnownUrlRef.current)
          }}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault()
              handleSubmit()
              e.currentTarget.blur()
            } else if (e.key === 'Escape') {
              e.currentTarget.blur()
            }
          }}
          className="flex-1 min-w-0 bg-transparent border border-[var(--color-border)] px-2 py-0.5 text-[11px] text-[var(--color-text)] font-mono outline-none focus:border-[var(--color-text-muted)]"
        />
        <div className="flex h-full shrink-0">
        <button
          type="button"
          onClick={handleReload}
          title="Reload"
          aria-label="Reload"
          className="flex h-full w-9 flex-shrink-0 items-center justify-center text-[var(--color-text-muted)] transition-colors hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-secondary)]"
        >
          <svg
            xmlns="http://www.w3.org/2000/svg"
            width="14"
            height="14"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
            aria-hidden="true"
          >
            <path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" />
            <path d="M3 3v5h5" />
            <path d="M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16" />
            <path d="M16 16h5v5" />
          </svg>
        </button>
        {import.meta.env.DEV && (
          <button
            className="flex h-full w-9 flex-shrink-0 items-center justify-center text-[var(--color-text-muted)] transition-colors hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-secondary)]"
            onClick={() =>
              void invoke('browser_devtools', { itemId, parentWindow }).catch(() => {})
            }
            title="Open devtools (dev builds)"
            aria-label="Open devtools"
          >
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <circle cx="12" cy="12" r="3" />
              <path d="M19.4 15a1.65 1.65 0 00.33 1.82l.06.06a2 2 0 010 2.83 2 2 0 01-2.83 0l-.06-.06a1.65 1.65 0 00-1.82-.33 1.65 1.65 0 00-1 1.51V21a2 2 0 01-4 0v-.09A1.65 1.65 0 009 19.4a1.65 1.65 0 00-1.82.33l-.06.06a2 2 0 01-2.83-2.83l.06-.06A1.65 1.65 0 004.68 15a1.65 1.65 0 00-1.51-1H3a2 2 0 010-4h.09A1.65 1.65 0 004.6 9a1.65 1.65 0 00-.33-1.82l-.06-.06a2 2 0 012.83-2.83l.06.06A1.65 1.65 0 009 4.68a1.65 1.65 0 001-1.51V3a2 2 0 014 0v.09a1.65 1.65 0 001 1.51 1.65 1.65 0 001.82-.33l.06-.06a2 2 0 012.83 2.83l-.06.06A1.65 1.65 0 0019.4 9a1.65 1.65 0 001.51 1H21a2 2 0 010 4h-.09a1.65 1.65 0 00-1.51 1z" />
            </svg>
          </button>
        )}
        </div>
      </div>

      {/* Mid-session errors (bad scheme, navigate failure): the native
          view hides any content-area DOM, so surface them in a strip.
          The strip resizes the dock area, which the ResizeObserver
          bounds-push absorbs automatically. */}
      {error && created && (
        <div className="border-b border-[var(--color-border)] bg-[var(--color-bg-stripe)] px-3 py-1 flex-shrink-0 text-[10px] text-[var(--color-status-error-text)]">
          {error}
        </div>
      )}

      {/* Docking area — the native child webview is positioned exactly
          over this div. DOM content here only shows when the view is
          absent (not created / stub build / error). */}
      <div ref={contentRef} className="flex-1 min-h-0 relative">
        {placeholder !== null && (
          <div
            className="absolute inset-0 flex items-center justify-center px-4 text-center text-[var(--color-text-muted)]"
            style={{ fontSize: '11px' }}
          >
            {placeholder}
          </div>
        )}
      </div>
    </div>
  )
}
