// The call's view actions, each named for what it does:
// - Expand (the floating tile, the top bar's slim bar): the call's stage in
//   its place, kept until Float or the call ends.
// - Float (the stage): back to the floating tile while the call has video,
//   and to the top bar's slim bar while it hasn't.
// - Pop out: the call in its own window.
// - Full screen: the call over K2, inside this window (CallExpanded), and
//   Exit full screen to come back. The OS's own full screen is the call
//   window's green button.
// - Back to chat (in the call's own window): the call goes back into the
//   window it came from, and the call's window closes.
// Minimising is the OS window's own; there is no Hide and no Bigger. The
// tile's and the zoom's buttons on a picture use PictureButton.

import { ArrowLeftIcon, ExpandIcon, PopInIcon, PopOutIcon } from '@/components/icons/K2Icons'
import { Button } from '@/components/ui/Button'
import { effectiveKeybinding, formatKeyCombo } from '@shared/hotkeys'
import { isMacPlatform } from '@/lib/desktop-chrome'
import { CALL_EXPAND_HOTKEY } from '@/lib/voice/call-view'
import { useSettingsStore } from '@/stores/settings'
import { expandCall, shrinkCall, useCallExpand } from '@/stores/call-expand'
import { floatCall } from '@/stores/call-place'
import { popOutCall, useCallPopout } from '@/stores/call-popout'
import { CALL_PANEL_ICON, CallPanelButton } from './CallControls'
import CallBarWord from './CallBarWord'

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

/** Float, on the stage: the call goes back to following what it carries,
 *  the floating tile with video (`video`), the top bar's slim bar without. */
export function FloatButton({ testId, video }: { testId: string; video: boolean }): React.JSX.Element {
  const title = video ? 'Float: the call in a small tile in a corner' : 'Float: the call back in the top bar'
  return (
    <Button variant="quiet" data-testid={testId} aria-label="Float" title={title} className="flex items-center" style={noDrag} onClick={floatCall}>
      <CallBarWord icon={<ExpandIcon shrink size={14} />} word="Float" />
    </Button>
  )
}

/** Expand, on the top bar's slim bar: the call's stage in its place. `back`:
 *  the stage is already the person's choice and only its conversation is
 *  away, so it reads Back to call. */
export function ExpandBarButton({ onPress, back = false }: { onPress: () => void; back?: boolean }): React.JSX.Element {
  if (back) {
    return (
      <Button variant="quiet" data-testid="call-bar-back" aria-label="Back to call" title="Back to call: open the conversation the call is in" className="flex items-center" style={noDrag} onClick={onPress}>
        <CallBarWord icon={<ArrowLeftIcon size={14} />} word="Back to call" />
      </Button>
    )
  }
  return (
    <Button variant="quiet" data-testid="call-bar-expand" aria-label="Expand" title="Expand: show the call's stage in its place" className="flex items-center" style={noDrag} onClick={onPress}>
      <CallBarWord icon={<ExpandIcon size={14} />} word="Expand" />
    </Button>
  )
}

/** Pop out, on the top bar's slim bar, in words. Nothing where the app
 *  can't (the web client). */
export function PopOutBarButton(): React.JSX.Element | null {
  const available = useCallPopout((s) => s.available)
  if (!available) return null
  return (
    <Button variant="quiet" data-testid="call-bar-popout" aria-label="Pop out" title="Pop out: the call in its own window" className="flex items-center" style={noDrag} onClick={() => void popOutCall()}>
      <CallBarWord icon={<PopOutIcon size={14} />} word="Pop out" />
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
