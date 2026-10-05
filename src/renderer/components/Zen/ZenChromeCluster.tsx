// prd-zen-mode-v1 Z25, Z63 — Linux and Windows keep K2's window controls in
// Zen. Those windows are frameless, so with no K2 top bar there would be no
// close / minimise / maximise. K2 draws a small reserved cluster in the
// platform's corner (Linux squares on the left, Windows controls on the
// right) plus a Menu button that opens the app menu (which has Enter/Exit
// Zen Mode). The page can't remove or cover it: its rect is reserved and
// the required-controls check refuses a control under it. macOS draws
// nothing here (the system stoplights stay).

import { useLayoutEffect, useRef, useState } from 'react'
import AppMenuPanel from '@/components/TopBar/AppMenuPanel'
import LinuxStoplights from '@/components/TopBar/LinuxStoplights'
import WindowControls from '@/components/TopBar/WindowControls'
import type { ZenRect } from '@/lib/zen/zen-controls'
import type { DesktopOs } from '@/lib/desktop-chrome'

export function zenClusterSide(os: DesktopOs): 'left' | 'right' | null {
  if (os === 'linux') return 'left'
  if (os === 'windows') return 'right'
  return null
}

function MenuButton({ side }: { side: 'left' | 'right' }): React.JSX.Element {
  const [open, setOpen] = useState(false)
  return (
    <div className="relative no-drag" style={{ height: 28 }}>
      <button
        type="button"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label="Menu"
        title="Menu"
        data-zen-app-menu=""
        onClick={() => setOpen((v) => !v)}
        className="flex h-full items-center px-2 cursor-pointer"
        style={{ color: 'var(--zen-text-muted)' }}
      >
        <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden>
          <path d="M2 4h12M2 8h12M2 12h12" strokeLinecap="round" />
        </svg>
      </button>
      {open && (
        <>
          <div className="fixed inset-0" style={{ zIndex: 1 }} onClick={() => setOpen(false)} />
          <div className="absolute" style={{ top: '100%', [side]: 0, width: 220, height: 0, zIndex: 2 }}>
            <AppMenuPanel onClose={() => setOpen(false)} />
          </div>
        </>
      )}
    </div>
  )
}

/** K2's window controls in Zen (Linux / Windows). Reports its rect. */
export function ZenChromeCluster({
  os,
  onRect,
}: {
  os: DesktopOs
  onRect(rect: ZenRect | null): void
}): React.JSX.Element | null {
  const side = zenClusterSide(os)
  const ref = useRef<HTMLDivElement | null>(null)
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) {
      onRect(null)
      return
    }
    const measure = (): void => {
      const r = el.getBoundingClientRect()
      onRect({ left: r.left, top: r.top, width: r.width, height: r.height })
    }
    measure()
    let ro: ResizeObserver | null = null
    if (typeof ResizeObserver !== 'undefined') {
      ro = new ResizeObserver(measure)
      ro.observe(el)
    }
    window.addEventListener('resize', measure)
    return () => {
      ro?.disconnect()
      window.removeEventListener('resize', measure)
    }
  }, [onRect, side])
  if (!side) return null
  return (
    <div
      ref={ref}
      data-zen-chrome-cluster={side}
      className="no-drag absolute top-0 flex items-center gap-1"
      style={{ [side]: 0, height: 36, padding: '0 6px', zIndex: 3 }}
    >
      {side === 'left' ? (
        <>
          <LinuxStoplights />
          <MenuButton side="left" />
        </>
      ) : (
        <>
          <MenuButton side="right" />
          <WindowControls />
        </>
      )}
    </div>
  )
}
