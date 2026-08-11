import { useCallback, useEffect, useState, type CSSProperties } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { getDesktopChrome } from '@/lib/desktop-chrome'
import { TOPBAR_HEIGHT } from '../../../shared/constants'

/**
 * Custom window controls for frameless Win/Linux chrome.
 * Full top-bar height (hover fill); width 24px (Rosson). Small pad after close.
 * Hover via normal Tailwind classes (globals @custom-variant hover fixes WebView2).
 */
export default function WindowControls(): React.JSX.Element | null {
  const chrome = getDesktopChrome()
  const [maximized, setMaximized] = useState(false)

  useEffect(() => {
    if (!chrome.windowControls) return
    const win = getCurrentWindow()
    void win.isMaximized().then(setMaximized).catch(() => {})
    let unlistenResize: (() => void) | undefined
    void win
      .listen('tauri://resize', () => {
        void win.isMaximized().then(setMaximized).catch(() => {})
      })
      .then((fn) => {
        unlistenResize = fn
      })
      .catch(() => {})
    return () => {
      unlistenResize?.()
    }
  }, [chrome.windowControls])

  const minimize = useCallback(() => {
    void getCurrentWindow().minimize().catch(() => {})
  }, [])

  const toggleMax = useCallback(() => {
    const win = getCurrentWindow()
    void win
      .isMaximized()
      .then((m) => (m ? win.unmaximize() : win.maximize()))
      .then(() => win.isMaximized())
      .then(setMaximized)
      .catch(() => {})
  }, [])

  const close = useCallback(() => {
    void getCurrentWindow().close().catch(() => {})
  }, [])

  if (!chrome.windowControls) return null

  // Full TOPBAR_HEIGHT so hover fills the bar vertically (items-center parents
  // would otherwise shrink buttons to icon height).
  const clusterStyle: CSSProperties = {
    display: 'flex',
    flexShrink: 0,
    height: TOPBAR_HEIGHT,
    alignItems: 'stretch',
    // Small gap past close (was 8; Rosson: ~40% less → 5).
    paddingRight: 0,
    // @ts-expect-error -- Electron/WebView app-region
    WebkitAppRegion: 'no-drag',
  }

  const btnStyle: CSSProperties = {
    display: 'flex',
    width: 24,
    height: '100%',
    alignItems: 'center',
    justifyContent: 'center',
    border: 'none',
    padding: 0,
    margin: 0,
    cursor: 'default',
    // @ts-expect-error -- Electron/WebView app-region
    WebkitAppRegion: 'no-drag',
  }

  return (
    <div className="no-drag" style={clusterStyle}>
      <button
        type="button"
        className="no-drag text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text-primary)] transition-colors"
        style={btnStyle}
        onClick={minimize}
        aria-label="Minimize"
        title="Minimize"
      >
        <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.2">
          <line x1="1" y1="5" x2="9" y2="5" />
        </svg>
      </button>
      <button
        type="button"
        className="no-drag text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] hover:text-[var(--color-text-primary)] transition-colors"
        style={btnStyle}
        onClick={toggleMax}
        aria-label={maximized ? 'Restore' : 'Maximize'}
        title={maximized ? 'Restore' : 'Maximize'}
      >
        {maximized ? (
          <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.2">
            <rect x="2.5" y="1" width="6.5" height="6.5" />
            <polyline points="1,3.5 1,9 6.5,9" />
          </svg>
        ) : (
          <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.2">
            <rect x="1.5" y="1.5" width="7" height="7" />
          </svg>
        )}
      </button>
      <button
        type="button"
        className="no-drag text-[var(--color-text-secondary)] hover:bg-[#e81123] hover:text-white transition-colors"
        style={btnStyle}
        onClick={close}
        aria-label="Close"
        title="Close"
      >
        <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.2">
          <line x1="1.5" y1="1.5" x2="8.5" y2="8.5" />
          <line x1="8.5" y1="1.5" x2="1.5" y2="8.5" />
        </svg>
      </button>
    </div>
  )
}
