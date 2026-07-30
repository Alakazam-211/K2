import React, { useCallback, useEffect, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useIsTabVisible } from '@/contexts/TabVisibilityContext'
import { usePageViewStore } from '@/stores/page-view'
import { useSettingsStore } from '@/stores/settings'
import { useTabsStore } from '@/stores/tabs'
import { webFeatures } from '@/web/features'

/**
 * Embedded Browser Tab pane (PRD .k2/prds/prd-browser-pane-v1.md).
 *
 * The browsed page does NOT render in this component — it lives in a
 * NATIVE child webview owned by src-tauri (commands/browser_webviews.rs,
 * label `browser-<itemId>`), which floats over the DOM unconditionally.
 * This component is the DOM-side "docking frame":
 *
 *  - bounds bridge: a ResizeObserver on the content area + a window
 *    resize listener feed a rAF-throttled `browser_set_bounds`, plus a
 *    150ms settle re-assert after window resizes (tauri #10131/#14843
 *    manifest as stale child bounds after resize/restore).
 *  - visibility bridge: `useIsTabVisible()` (tab visible AND this item
 *    active in its pane group) plus Settings / full-page overlays drive
 *    `browser_set_visible` — native child webviews float OVER the DOM, so
 *    hiding the workspace with `display:none` is not enough (Settings
 *    was showing Google sign-in still painted on Companion).
 *  - lifecycle: the webview is created lazily on FIRST visibility (so a
 *    restored background layout doesn't load pages invisibly) and
 *    `browser_close`d on unmount (item/tab close).
 *
 * Chrome is deliberately minimal: address field (Enter → navigate),
 * reload, devtools (dev builds). No back/forward — the Rust command
 * surface has no history API.
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

/** Prefix bare hostnames with https:// so "example.com" just works. */
function normalizeUrl(raw: string): string {
  const trimmed = raw.trim()
  if (!trimmed) return ''
  if (/^https?:\/\//i.test(trimmed)) return trimmed
  return `https://${trimmed}`
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
  const visible = standalone ? true : tabVisible && !workspaceCovered
  const setBrowserItemState = useTabsStore((s) => s.setBrowserItemState)

  // Content area the native view docks onto (below the chrome bar).
  const contentRef = useRef<HTMLDivElement | null>(null)

  const [created, setCreated] = useState(false)
  const createdRef = useRef(false)
  /** Feature-off stub build / hosted web → render the graceful-degradation message. */
  const [unavailable, setUnavailable] = useState(!webFeatures.browserPane)
  const [error, setError] = useState<string | null>(null)

  // Address bar. Mirrors the polled current URL unless focused (don't
  // clobber the user's in-progress edit).
  const [address, setAddress] = useState(url)
  const addressFocusedRef = useRef(false)

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
    // webview fills the window at origin, so this IS the main-window
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
    pendingUrlRef.current = normalized
    const rect = measureRect()
    if (!rect) {
      // Dock not laid out yet (common in Settings full-pane embeds). ResizeObserver
      // will retry when the content area gets a real size — do NOT clear pending.
      return
    }
    setError(null)
    try {
      await invoke('browser_create', { itemId, url: normalized, rect })
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
    }
  }, [itemId, measureRect])

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
      void invoke('browser_set_bounds', { itemId, rect }).catch(() => {
        // View may have been closed in a race; the visibility/lifecycle
        // effects own recovery.
      })
    })
  }, [itemId, measureRect, createView])

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

  const navigateView = useCallback(async (targetUrl: string): Promise<void> => {
    setError(null)
    try {
      await invoke('browser_navigate', { itemId, url: targetUrl })
      lastKnownUrlRef.current = targetUrl
    } catch (e) {
      setError(String(e))
    }
  }, [itemId])

  // ── Visibility bridge + lazy creation ───────────────────────────────
  useEffect(() => {
    if (visible) {
      if (!createdRef.current) {
        const target = normalizeUrl(url)
        if (target) void createView(target)
      } else {
        void invoke('browser_set_visible', { itemId, visible: true }).catch(() => {})
        // Bounds may have gone stale while hidden (mosaic resizes under
        // display:none don't reach the native view) — re-assert on show.
        scheduleBoundsPush()
      }
    } else if (createdRef.current) {
      void invoke('browser_set_visible', { itemId, visible: false }).catch(() => {})
    }
  }, [visible, url, itemId, createView, scheduleBoundsPush])

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
  useEffect(() => {
    return () => {
      if (createdRef.current) {
        createdRef.current = false
        void invoke('browser_close', { itemId }).catch(() => {})
      }
    }
  }, [itemId])

  // ── Address-bar sync: poll current URL only while visible ──────────
  useEffect(() => {
    if (!visible || !created) return
    const timer = setInterval(() => {
      void (async () => {
        try {
          const current = await invoke<string>('browser_current_url', { itemId })
          if (!current || current === lastKnownUrlRef.current) return
          lastKnownUrlRef.current = current
          if (!addressFocusedRef.current) setAddress(current)
          // Stamp the store so serialize captures in-page navigation.
          // Standalone embeds (Settings OAuth) are not tab items.
          if (!standalone) {
            setBrowserItemState(tabId, paneGroupId, itemId, { url: current })
          }
        } catch {
          // View gone (close race) — the interval is cleared by unmount.
        }
      })()
    }, 1500)
    return () => clearInterval(timer)
  }, [visible, created, itemId, tabId, paneGroupId, setBrowserItemState, standalone])

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

  // ── Render ──────────────────────────────────────────────────────────
  const chromeButtonClass =
    'px-1.5 py-0.5 text-[11px] text-[var(--color-text-muted)] hover:text-[var(--color-text)] flex-shrink-0'

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
      <div className="flex items-center gap-2 border-b border-[var(--color-border)] bg-[var(--color-bg-stripe)] px-3 py-1.5 flex-shrink-0">
        <input
          type="text"
          value={address}
          spellCheck={false}
          autoCorrect="off"
          autoCapitalize="off"
          placeholder="Enter URL…"
          onChange={(e) => setAddress(e.target.value)}
          onFocus={(e) => {
            addressFocusedRef.current = true
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
          className="flex-1 min-w-0 bg-transparent border border-[var(--color-border)] rounded px-2 py-0.5 text-[11px] text-[var(--color-text)] font-mono outline-none focus:border-[var(--color-text-muted)]"
        />
        <button className={chromeButtonClass} onClick={handleReload} title="Reload">
          ⟳
        </button>
        {import.meta.env.DEV && (
          <button
            className={chromeButtonClass}
            onClick={() => void invoke('browser_devtools', { itemId }).catch(() => {})}
            title="Open devtools (dev builds)"
          >
            ⚙
          </button>
        )}
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
