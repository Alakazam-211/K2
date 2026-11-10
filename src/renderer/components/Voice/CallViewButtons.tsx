// Expand, Pop out and Pop in (Rosson 2026-11-10): the ways to give the call's
// video more room, and to bring it back. Expand opens the call over K2, inside
// the app (CallExpanded; it was OS full screen); Pop out moves it to its own
// window, and that window's Pop in brings it back. On the picture they're
// small icon buttons on the same chip as its labels; in a panel's header or a
// control bar, the strip's square buttons.

import { ExpandIcon, PopInIcon, PopOutIcon } from '@/components/icons/K2Icons'
import { effectiveKeybinding, formatKeyCombo } from '@shared/hotkeys'
import { isMacPlatform } from '@/lib/desktop-chrome'
import { CALL_EXPAND_HOTKEY } from '@/lib/voice/call-view'
import { useSettingsStore } from '@/stores/settings'
import { expandCall, shrinkCall, useCallExpand } from '@/stores/call-expand'
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

/** The Expand chord as this person has it bound, painted for this OS. */
export function useExpandChordLabel(): string | null {
  const keybindings = useSettingsStore((s) => s.keybindings)
  const mac = isMacPlatform()
  const combo = effectiveKeybinding(keybindings, CALL_EXPAND_HOTKEY, mac)
  if (!combo) return null
  return mac ? formatKeyCombo(combo, true) : combo
}

/** The label with the chord, for the tooltip ("Expand (⌃⌘V)"). */
export function expandLabel(chord: string | null): string {
  return chord ? `Expand (${chord})` : 'Expand'
}

/** Expand, or Shrink while expanded. `look`: on the picture, or the strip's
 *  square button. */
export function ExpandButton({ testId, look, chord }: { testId: string; look: 'picture' | 'panel'; chord: string | null }): React.JSX.Element {
  const on = useCallExpand((s) => s.expanded)
  const label = on ? 'Shrink (Esc)' : expandLabel(chord)
  const press = (): void => (on ? shrinkCall() : expandCall(testId))
  if (look === 'picture') {
    return (
      <PictureButton label={label} testId={testId} onPress={press}>
        <ExpandIcon shrink={on} size={13} />
      </PictureButton>
    )
  }
  return (
    <CallPanelButton label={label} testId={testId} onPress={press}>
      <ExpandIcon shrink={on} size={CALL_PANEL_ICON} />
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

/** Pop in, in the call's own window: back into K2's window, and this one
 *  closes. Pop out's square button. */
export function PopInButton({ testId, onPress }: { testId: string; onPress: () => void }): React.JSX.Element {
  return (
    <CallPanelButton label="Pop back into K2's window" testId={testId} onPress={onPress}>
      <PopInIcon size={CALL_PANEL_ICON} />
    </CallPanelButton>
  )
}
