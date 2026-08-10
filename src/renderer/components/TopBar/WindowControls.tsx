import { useCallback, useEffect, useState } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { getDesktopChrome } from '@/lib/desktop-chrome'

const btnClass =
  'flex h-full w-[46px] items-center justify-center text-[var(--color-text-secondary)] hover:bg-white/[0.08] hover:text-[var(--color-text-primary)] transition-colors no-drag'

export default function WindowControls(): React.JSX.Element | null {
  const chrome = getDesktopChrome()
  const [maximized, setMaximized] = useState(false)

  useEffect(() => {
    if (!chrome.windowControls) return
    const win = getCurrentWindow()
    void win.isMaximized().then(setMaximized).catch(() => {})
    let unlistenResize: (() => void) | undefined
    let unlistenFocus: (() => void) | undefined
    void win
      .listen('tauri://resize', () => {
        void win.isMaximized().then(setMaximized).catch(() => {})
      })
      .then((fn) => {
        unlistenResize = fn
      })
      .catch(() => {})
    void win
      .listen('tauri://focus', () => {
        void win.isMaximized().then(setMaximized).catch(() => {})
      })
      .then((fn) => {
        unlistenFocus = fn
      })
      .catch(() => {})
    return () => {
      unlistenResize?.()
      unlistenFocus?.()
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

  return (
    <div
      className="flex h-full items-stretch flex-shrink-0 no-drag"
      style={{
        // @ts-expect-error -- Electron-specific CSS property
        WebkitAppRegion: 'no-drag',
      }}
    >
      <button type="button" className={btnClass} onClick={minimize} aria-label="Minimize" title="Minimize">
        <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.2">
          <line x1="1" y1="5" x2="9" y2="5" />
        </svg>
      </button>
      <button
        type="button"
        className={btnClass}
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
        className={`${btnClass} hover:bg-[#e81123] hover:text-white`}
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
