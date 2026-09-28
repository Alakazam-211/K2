import { useEffect } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { isMacPlatform } from '@/lib/desktop-chrome'
import { isWebClient } from '@/lib/is-web'
import { decideNewWindowShortcut } from '@/lib/new-window-shortcut'

/**
 * Window-level New Window chord. useTerminalShortcuts is mounted only
 * while TerminalArea is, and its Shift+N return must stay (that path
 * is Cmd+N, and Ctrl+Shift+N never enters it).
 */
export function useNewWindowShortcut(): void {
  useEffect(() => {
    if (isWebClient()) return
    const handler = (e: KeyboardEvent): void => {
      if (e.repeat) return
      const action = decideNewWindowShortcut({
        key: e.key,
        metaKey: e.metaKey,
        ctrlKey: e.ctrlKey,
        altKey: e.altKey,
        shiftKey: e.shiftKey,
        isMac: isMacPlatform(),
      })
      if (action !== 'window_new') return
      e.preventDefault()
      e.stopPropagation()
      void invoke('window_new').catch(() => {})
    }
    window.addEventListener('keydown', handler, true)
    return () => window.removeEventListener('keydown', handler, true)
  }, [])
}
