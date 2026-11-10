// Full screen and Pop out (Rosson 2026-11-10): the two ways to give the
// call's video more room. On the picture they're small icon buttons on the
// same chip as its labels; in a panel's header or a control bar, the strip's
// square buttons. Both act at once, inside the click: full screen must be
// asked for in the person's own gesture.

import { FullscreenIcon, PopOutIcon } from '@/components/icons/K2Icons'
import { effectiveKeybinding, formatKeyCombo } from '@shared/hotkeys'
import { isMacPlatform } from '@/lib/desktop-chrome'
import { CALL_FULLSCREEN_HOTKEY } from '@/lib/voice/call-view'
import { useSettingsStore } from '@/stores/settings'
import { enterCallFullscreen, exitCallFullscreen, useCallFullscreen } from '@/stores/call-fullscreen'
import { popOutCall, useCallPopout } from '@/stores/call-popout'
import { CALL_PANEL_ICON, CallPanelButton } from './CallControls'

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const noDrag = { WebkitAppRegion: 'no-drag' } as any

/** An icon button drawn on the picture (the labels' chip). */
const ON_PICTURE =
  'flex h-6 w-6 items-center justify-center bg-[var(--color-media-label-bg)] text-[var(--color-media-label-text)] opacity-80 transition-opacity hover:opacity-100 focus-visible:opacity-100'

export function PictureButton({
  label,
  testId,
  onPress,
  children,
}: {
  label: string
  testId: string
  onPress: () => void
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <button type="button" aria-label={label} title={label} data-testid={testId} className={ON_PICTURE} style={noDrag} onClick={onPress}>
      {children}
    </button>
  )
}

/** The full-screen chord as this person has it bound, painted for this OS. */
export function useFullscreenChordLabel(): string | null {
  const keybindings = useSettingsStore((s) => s.keybindings)
  const mac = isMacPlatform()
  const combo = effectiveKeybinding(keybindings, CALL_FULLSCREEN_HOTKEY, mac)
  if (!combo) return null
  return mac ? formatKeyCombo(combo, true) : combo
}

/** The label with the chord, for the tooltip ("Full screen (⌃⌘V)"). */
export function fullscreenLabel(chord: string | null): string {
  return chord ? `Full screen (${chord})` : 'Full screen'
}

/** Full screen, or Exit full screen while in it. `look`: on the picture, or
 *  the strip's square button. */
export function FullscreenButton({ testId, look, chord }: { testId: string; look: 'picture' | 'panel'; chord: string | null }): React.JSX.Element {
  const on = useCallFullscreen((s) => s.mode !== 'off')
  const label = on ? 'Exit full screen (Esc)' : fullscreenLabel(chord)
  const press = (): void => (on ? exitCallFullscreen() : enterCallFullscreen())
  if (look === 'picture') {
    return (
      <PictureButton label={label} testId={testId} onPress={press}>
        <FullscreenIcon exit={on} size={13} />
      </PictureButton>
    )
  }
  return (
    <CallPanelButton label={label} testId={testId} onPress={press}>
      <FullscreenIcon exit={on} size={CALL_PANEL_ICON} />
    </CallPanelButton>
  )
}

/** Pop out: the call in its own window. Nothing where the app can't (the web
 *  client). */
export function PopOutButton({ testId, look }: { testId: string; look: 'picture' | 'panel' }): React.JSX.Element | null {
  const available = useCallPopout((s) => s.available)
  if (!available) return null
  const label = 'Pop out into its own window'
  const press = (): void => void popOutCall()
  if (look === 'picture') {
    return (
      <PictureButton label={label} testId={testId} onPress={press}>
        <PopOutIcon size={13} />
      </PictureButton>
    )
  }
  return (
    <CallPanelButton label={label} testId={testId} onPress={press}>
      <PopOutIcon size={CALL_PANEL_ICON} />
    </CallPanelButton>
  )
}
