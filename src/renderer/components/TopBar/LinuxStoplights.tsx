import { useCallback, useEffect, useState, type CSSProperties } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { useStyleStore } from '@/stores/style'
import { useWindowFocusStore } from '@/stores/window-focus'
import {
  LINUX_STOPLIGHT_GAP_PX,
  LINUX_STOPLIGHT_HIT_PX,
  LINUX_STOPLIGHT_NUDGE_Y_PX,
  LINUX_STOPLIGHT_RADIUS_PX,
  LINUX_STOPLIGHT_SQUARE_PX,
  linuxStoplightFill,
  linuxStoplightRolesForDesktop,
  type LinuxStoplightRole,
} from '@/lib/linux-stoplights'

const noDrag = { WebkitAppRegion: 'no-drag' } as CSSProperties

/**
 * In-flow square stoplights for Linux. GNOME shows close, minimize,
 * and maximize. Every other session shows close only, until the
 * session name arrives. Actions are this window's close / minimize /
 * maximize. WindowControls stays hidden (windowControls false).
 */
export default function LinuxStoplights(): React.JSX.Element {
  const focused = useWindowFocusStore((s) => s.isFocused)
  const dark = useStyleStore((s) => s.resolvedScheme) !== 'light'
  const [maximized, setMaximized] = useState(false)
  const [roles, setRoles] = useState<readonly LinuxStoplightRole[]>(['close'])

  useEffect(() => {
    let cancelled = false
    void invoke<string>('linux_desktop_session')
      .then((desktop) => {
        if (!cancelled) setRoles(linuxStoplightRolesForDesktop(desktop))
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [])

  useEffect(() => {
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
  }, [])

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

  const run: Record<LinuxStoplightRole, () => void> = {
    close,
    minimize,
    maximize: toggleMax,
  }

  const label = (role: LinuxStoplightRole): string => {
    if (role === 'close') return 'Close'
    if (role === 'minimize') return 'Minimize'
    return maximized ? 'Restore' : 'Maximize'
  }

  return (
    <div
      className="no-drag flex shrink-0 items-center"
      style={{
        gap: LINUX_STOPLIGHT_GAP_PX,
        transform: `translateY(${LINUX_STOPLIGHT_NUDGE_Y_PX}px)`,
        ...noDrag,
      }}
      aria-label="Window controls"
    >
      {roles.map((role) => (
        <button
          key={role}
          type="button"
          className="no-drag flex shrink-0 items-center justify-center p-0"
          style={{
            width: LINUX_STOPLIGHT_HIT_PX,
            height: LINUX_STOPLIGHT_HIT_PX,
            border: 'none',
            background: 'transparent',
            borderRadius: LINUX_STOPLIGHT_RADIUS_PX,
            cursor: 'default',
            ...noDrag,
          }}
          onClick={run[role]}
          aria-label={label(role)}
          title={label(role)}
        >
          <span
            aria-hidden
            style={{
              width: LINUX_STOPLIGHT_SQUARE_PX,
              height: LINUX_STOPLIGHT_SQUARE_PX,
              display: 'block',
              background: linuxStoplightFill(role, { focused, dark }),
              borderRadius: LINUX_STOPLIGHT_RADIUS_PX,
            }}
          />
        </button>
      ))}
    </div>
  )
}
