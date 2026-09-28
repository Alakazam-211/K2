import { useEffect, useRef, useState } from 'react'
import { openUrl } from '@tauri-apps/plugin-opener'
import k2Logo from '../../assets/k2-logo.png'
import { desktopOsFromNavigator, logoOpensAppMenu } from '@/lib/desktop-chrome'
import { isWebClient } from '@/lib/is-web'
import AppMenuPanel from './AppMenuPanel'

const DASHBOARD_URL = 'https://k2.dev/dashboard'

/**
 * 16×16 K2 mark. macOS and hosted web open the dashboard.
 * Windows and Linux open the app menu from this icon (the relative
 * parent), not from a separate Menu button in the left slot.
 */
export default function K2MarkButton(): React.JSX.Element {
  const opensMenu = logoOpensAppMenu(
    isWebClient(),
    desktopOsFromNavigator(typeof navigator === 'undefined' ? null : navigator),
  )
  const [open, setOpen] = useState(false)
  const rootRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!opensMenu || !open) return
    const onDown = (e: MouseEvent): void => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false)
    }
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.stopPropagation()
        setOpen(false)
      }
    }
    document.addEventListener('mousedown', onDown)
    window.addEventListener('keydown', onKey, true)
    return () => {
      document.removeEventListener('mousedown', onDown)
      window.removeEventListener('keydown', onKey, true)
    }
  }, [open, opensMenu])

  return (
    <div ref={rootRef} className="relative flex-shrink-0 no-drag">
      <button
        type="button"
        onClick={() => {
          if (opensMenu) {
            setOpen((o) => !o)
            return
          }
          void openUrl(DASHBOARD_URL)
        }}
        className="no-drag flex h-4 w-4 items-center justify-center p-0 flex-shrink-0"
        style={{ WebkitAppRegion: 'no-drag' } as React.CSSProperties}
        aria-haspopup={opensMenu ? 'menu' : undefined}
        aria-expanded={opensMenu ? open : undefined}
        title={opensMenu ? 'Menu' : 'K2 dashboard'}
        aria-label={opensMenu ? 'Menu' : 'Open the K2 dashboard'}
      >
        <img src={k2Logo} alt="" className="h-4 w-4" />
      </button>
      {opensMenu && open && <AppMenuPanel onClose={() => setOpen(false)} />}
    </div>
  )
}
