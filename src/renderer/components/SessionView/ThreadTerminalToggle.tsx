import { useEffect, useRef, type JSX } from 'react'
import { useRoom } from '@/components/Room/RoomContext'
import { useSettingsStore } from '@/stores/settings'
import { isMacPlatform } from '@/lib/desktop-chrome'
import { effectiveKeybinding, formatKeyCombo } from '@shared/hotkeys'
import { registerRevealTarget } from './threadReveal'
import { floatProps } from './threadFloats'

/** Beside the view button, in Thread view only (prd-thread-view-focus-v1 §6).
 *  Mounted, it is also the target the Ctrl+` chord finds (`threadReveal.ts`). */
export function ThreadTerminalToggle({
  revealed,
  disabled,
  onToggle,
}: {
  revealed: boolean
  /** No terminal yet (the session is starting or has ended). */
  disabled: boolean
  onToggle: () => void
}): JSX.Element {
  const room = useRoom()
  const ref = useRef<HTMLButtonElement>(null)
  const latest = useRef({ disabled, onToggle })
  latest.current = { disabled, onToggle }
  const combo = useSettingsStore((s) => effectiveKeybinding(s.keybindings, 'toggleThreadTerminal', isMacPlatform()))

  useEffect(() => {
    const element = ref.current
    if (!element) return
    return registerRevealTarget({
      room,
      element,
      enabled: () => !latest.current.disabled,
      toggle: () => latest.current.onToggle(),
    })
  }, [room])

  const action = revealed ? 'Hide terminal' : 'Reveal terminal'
  const key = combo ? ` (${formatKeyCombo(combo, isMacPlatform())})` : ''

  return (
    <button
      ref={ref}
      type="button"
      data-testid="thread-terminal-toggle"
      {...floatProps(true, 'round')}
      aria-pressed={revealed}
      aria-label={action}
      title={disabled ? 'No terminal is running yet' : `${action}${key}`}
      disabled={disabled}
      // Keep focus where it is: a click must not pull it out of the terminal
      // before the hide decides where focus goes.
      onMouseDown={(e) => e.preventDefault()}
      onClick={onToggle}
      className="no-drag cursor-pointer inline-flex items-center justify-center flex-shrink-0 disabled:cursor-not-allowed"
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
        <path d="M5 8l5 4-5 4" />
        <path d="M12 18h7" />
      </svg>
    </button>
  )
}
