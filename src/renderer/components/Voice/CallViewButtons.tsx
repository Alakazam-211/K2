// The call's view actions, each named for what it does:
// - Pop out: the call in its own window.
// - Full screen: the call over K2, inside this window (CallExpanded), and
//   Exit full screen to come back. The OS's own full screen is the call
//   window's green button.
// - Compact / Expand: one toggle, the stage folded into the slim in-call bar
//   and opened again.
// - Back to chat (in the call's own window): the call goes back into the
//   window it came from, and the call's window closes.
// Minimising is the OS window's own; there is no Hide and no Bigger. The
// zoom's buttons on a picture use PictureButton.

import { ExpandIcon, PopInIcon, PopOutIcon } from '@/components/icons/K2Icons'
import { Button } from '@/components/ui/Button'
import { effectiveKeybinding, formatKeyCombo } from '@shared/hotkeys'
import { isMacPlatform } from '@/lib/desktop-chrome'
import { CALL_EXPAND_HOTKEY } from '@/lib/voice/call-view'
import { useSettingsStore } from '@/stores/settings'
import { expandCall, shrinkCall, useCallExpand } from '@/stores/call-expand'
import { compactCallStage, expandCallStage } from '@/stores/call-place'
import { popOutCall, useCallPopout } from '@/stores/call-popout'
import { CALL_PANEL_ICON, CallPanelButton } from './CallControls'

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const noDrag = { WebkitAppRegion: 'no-drag' } as any

/** An icon button drawn on the picture (the labels' chip): a circle in
 *  Glass and the chip's cut in Terminal, with the shared focus ring. */
const ON_PICTURE =
  'k2-focus flex h-6 w-6 items-center justify-center rounded-[var(--radius-pill)] bg-[var(--color-media-label-bg)] text-[var(--color-media-label-text)] opacity-80 transition-opacity hover:opacity-100 focus-visible:opacity-100'

/** The chip's Terminal cut with no edge drawn on it (the chip has none). */
const ON_PICTURE_CUT = { WebkitAppRegion: 'no-drag', '--k2-ch-w': '0px' } as React.CSSProperties

export function PictureButton({
  label,
  testId,
  onPress,
  disabled = false,
  wide = false,
  children,
}: {
  label: string
  testId: string
  onPress: () => void
  disabled?: boolean
  /** A word, not an icon (the zoom's Fit): as wide as it needs. */
  wide?: boolean
  children: React.ReactNode
}): React.JSX.Element {
  const cls = wide ? ON_PICTURE.replace('w-6', 'w-auto px-1.5 text-[11px] leading-[14px]') : ON_PICTURE
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      data-testid={testId}
      disabled={disabled}
      data-k2-chamfer="chip"
      className={`${cls} disabled:opacity-40`}
      style={ON_PICTURE_CUT}
      onClick={onPress}
    >
      {children}
    </button>
  )
}

/** The Full screen chord as this person has it bound, painted for this OS. */
export function useFullScreenChordLabel(): string | null {
  const keybindings = useSettingsStore((s) => s.keybindings)
  const mac = isMacPlatform()
  const combo = effectiveKeybinding(keybindings, CALL_EXPAND_HOTKEY, mac)
  if (!combo) return null
  return mac ? formatKeyCombo(combo, true) : combo
}

/** The label with the chord, for the tooltip ("Full screen (⌃⌘V)"). */
export function fullScreenLabel(chord: string | null): string {
  return chord ? `Full screen (${chord})` : 'Full screen'
}

/** Full screen, or Exit full screen while it's open. */
export function FullScreenButton({ testId, chord }: { testId: string; chord: string | null }): React.JSX.Element {
  const on = useCallExpand((s) => s.expanded)
  const label = on ? 'Exit full screen (Esc)' : fullScreenLabel(chord)
  return (
    <CallPanelButton label={label} testId={testId} onPress={() => (on ? shrinkCall() : expandCall(testId))}>
      <ExpandIcon shrink={on} size={CALL_PANEL_ICON} />
    </CallPanelButton>
  )
}

/** Pop out: the call in its own window. Nothing where the app can't (the web
 *  client). */
export function PopOutButton({ testId }: { testId: string }): React.JSX.Element | null {
  const available = useCallPopout((s) => s.available)
  if (!available) return null
  return (
    <CallPanelButton label="Pop out into its own window" testId={testId} onPress={() => void popOutCall()}>
      <PopOutIcon size={CALL_PANEL_ICON} />
    </CallPanelButton>
  )
}

/** Compact (the stage folds into the slim bar) or Expand (it opens again):
 *  one toggle, in words. `compact`: the stage is folded now. */
export function CompactButton({ testId, compact }: { testId: string; compact: boolean }): React.JSX.Element {
  const word = compact ? 'Expand' : 'Compact'
  const title = compact ? 'Expand: show the call’s video here' : 'Compact: fold the call into a slim bar'
  return (
    <Button variant="quiet" data-testid={testId} title={title} style={noDrag} onClick={() => (compact ? expandCallStage() : compactCallStage())}>
      {word}
    </Button>
  )
}

/** Back to chat, in the call's own window: the call goes back into the
 *  window it came from, and this one closes. */
export function BackToChatButton({ testId, onPress }: { testId: string; onPress: () => void }): React.JSX.Element {
  return (
    <Button
      variant="quiet"
      data-testid={testId}
      title="Back to chat: the call goes back into the window it came from, and this one closes"
      className="flex items-center gap-1.5"
      style={noDrag}
      onClick={onPress}
    >
      <PopInIcon size={13} />
      Back to chat
    </Button>
  )
}
