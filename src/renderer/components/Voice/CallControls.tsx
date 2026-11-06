// The call's quick controls: Mute, Deafen, Camera and Share screen (with
// video rooms on) and Hang up. They sit in the top bar beside the headset while the call has no
// video, and under the floating video tile while it has.
//
// Ported from K2 Companion's call tile strip (src/zen/voice/callTile.tsx,
// `Controls` and `TileButton`): the same buttons, labels and states (a
// moderator's mute shows locked and does nothing; a camera the room forbids
// shows dimmed). K2's look: the top bar's icon buttons, and in the tile's
// strip K2's elevated panel buttons. Every press goes to the voice and video
// stores, which drive the engine; nothing here holds call state.

import type { ReactNode } from 'react'
import { useStore } from 'zustand'
import {
  HeadphonesIcon,
  MicIcon,
  MicOffIcon,
  PhoneOffIcon,
  ScreenShareIcon,
  TOP_BAR_ICON_REST_CLASS,
  VideoIcon,
} from '@/components/icons/K2Icons'
import { micBlockedText, MIC_DENIED_LINE } from '@/lib/voice/voice-wire'
import { policyBlockText } from '@/lib/voice/video-labels'
import type { VoiceCall } from '@/stores/voice-core'
import type { VideoController } from '@/stores/video-core'

export type CallControlsVariant = 'topbar' | 'tile'

/** The tile strip's button (px). */
export const CALL_TILE_BUTTON = 28
/** The icon inside a tile-strip or headset-list button (px). */
export const CALL_PANEL_ICON = 15

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const noDrag = { WebkitAppRegion: 'no-drag' } as any

const TOPBAR_ON = 'text-[var(--color-status-error)] hover:bg-[var(--color-bg-elevated)]'
const TOPBAR_DANGER = 'text-[var(--color-danger)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-danger-hover)]'
const TILE_BASE = 'border border-[var(--color-border)] bg-[var(--color-bg-elevated)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-surface)]'
/** Engaged (muted, deafened, the camera on): inverted, as Companion's white. */
const TILE_ON = 'border border-[var(--color-text-primary)] bg-[var(--color-text-primary)] text-[var(--color-bg)]'
/** Hang up: the app's danger color (as the danger Button). */
const TILE_DANGER =
  'border border-[var(--color-danger)] bg-[var(--color-danger)] text-[var(--color-on-accent)] hover:bg-[var(--color-danger-active)] hover:border-[var(--color-danger-active)]'

/** The headset list's call row (VoicePopover, VideoCallButtons): the tile
 *  strip's square buttons, but a real `disabled` (the list has no drag). */
export function CallPanelButton({
  label,
  title,
  on,
  danger,
  disabled,
  testId,
  onPress,
  children,
  ...data
}: {
  /** The action and its state ("Mute" / "Unmute"). */
  label: string
  /** The tooltip, when it should say more than the label (why it's blocked). */
  title?: string
  /** A toggle's state (muted, deafened, the camera on, sharing): the engaged
   *  look and aria-pressed. Leave it out on a one-shot button. */
  on?: boolean
  danger?: boolean
  disabled?: boolean
  testId: string
  onPress: () => void
  children: ReactNode
} & { [k: `data-${string}`]: string | undefined }): React.JSX.Element {
  const look = danger ? TILE_DANGER : on ? TILE_ON : TILE_BASE
  return (
    <button
      type="button"
      aria-label={label}
      // Toggles only (Mute, Deafen, Camera, Share); undefined on one-shots.
      aria-pressed={on}
      title={title ?? label}
      data-testid={testId}
      {...data}
      disabled={disabled}
      className={`flex shrink-0 items-center justify-center transition-colors disabled:cursor-default disabled:opacity-50 ${look}`}
      style={{ width: CALL_TILE_BUTTON, height: CALL_TILE_BUTTON, ...noDrag }}
      onClick={onPress}
    >
      {children}
    </button>
  )
}

function CallButton({
  variant,
  label,
  pressed,
  on,
  danger,
  dead,
  testId,
  state,
  onPress,
  children,
}: {
  variant: CallControlsVariant
  label: string
  pressed?: boolean
  /** Engaged: muted, deafened (warning look), or the camera on. */
  on?: boolean
  danger?: boolean
  /** Held by a moderator or the room: shows it, does nothing. */
  dead?: boolean
  testId: string
  state?: string
  onPress: () => void
  children: ReactNode
}): React.JSX.Element {
  const look =
    variant === 'topbar'
      ? `h-6 w-6 ${danger ? TOPBAR_DANGER : on ? TOPBAR_ON : TOP_BAR_ICON_REST_CLASS}`
      : danger
        ? TILE_DANGER
        : on
          ? TILE_ON
          : TILE_BASE
  return (
    <button
      type="button"
      // Not `disabled`: a disabled button gets no pointer events, and a drag
      // may start anywhere on the tile.
      aria-disabled={dead || undefined}
      aria-pressed={pressed}
      aria-label={label}
      title={label}
      data-testid={testId}
      data-state={state}
      data-on={on ? 'true' : 'false'}
      className={`relative flex shrink-0 items-center justify-center transition-colors ${look} ${dead ? 'opacity-50' : ''}`}
      style={variant === 'tile' ? { width: CALL_TILE_BUTTON, height: CALL_TILE_BUTTON, ...noDrag } : noDrag}
      onClick={() => {
        if (!dead) onPress()
      }}
    >
      {children}
    </button>
  )
}

export interface CallControlsProps {
  variant: CallControlsVariant
  call: VoiceCall
  /** The voice controller's actions (the app's, or a test's). */
  actions: { setMuted(m: boolean): void; setDeafened(d: boolean): void; leave(): Promise<void> }
  /** With video rooms on: the camera button. */
  video: VideoController | null
}

function CameraButton({ variant, video, busy }: { variant: CallControlsVariant; video: VideoController; busy: boolean }): React.JSX.Element | null {
  const availability = useStore(video.store, (s) => s.availability)
  const caps = useStore(video.store, (s) => s.caps)
  const local = useStore(video.store, (s) => s.local)
  const pending = useStore(video.store, (s) => s.pending.camera)
  const policy = useStore(video.store, (s) => s.policy)
  if (availability !== 'ready' || !caps?.camera) return null
  const on = local.camera !== 'off'
  // Turning it off is always allowed.
  const block = on ? null : policyBlockText(policy, 'camera')
  const asking = local.camera === 'asking'
  return (
    <CallButton
      variant={variant}
      label={block ?? (on ? 'Turn camera off' : 'Turn camera on')}
      pressed={on}
      on={on}
      dead={busy || pending || asking || block !== null}
      testId={`call-${variant}-camera`}
      state={local.camera}
      onPress={() => void video.setCamera(!on)}
    >
      <VideoIcon off={!on} size={variant === 'tile' ? 15 : undefined} />
    </CallButton>
  )
}

/** Share screen. Where the OS has its own picker (macOS 14+, Windows, the
 *  Linux portal) a press opens it. Where it only lists (older macOS), the
 *  tile's strip shows the list (ScreenPicker under it); the top bar leaves
 *  that to the headset's list, which has Pick. */
function ScreenButton({ variant, video, busy }: { variant: CallControlsVariant; video: VideoController; busy: boolean }): React.JSX.Element | null {
  const availability = useStore(video.store, (s) => s.availability)
  const caps = useStore(video.store, (s) => s.caps)
  const local = useStore(video.store, (s) => s.local)
  const pending = useStore(video.store, (s) => s.pending.screen)
  const policy = useStore(video.store, (s) => s.policy)
  if (availability !== 'ready' || !caps?.screen.supported) return null
  const on = local.screen !== 'off'
  const picker = caps.screen.systemPicker
  if (!on && !picker && !(variant === 'tile' && caps.screen.list)) return null
  const block = on ? null : policyBlockText(policy, 'screen')
  const asking = local.screen === 'asking'
  return (
    <CallButton
      variant={variant}
      label={block ?? (on ? 'Stop sharing' : 'Share screen')}
      pressed={on}
      on={on}
      dead={busy || pending || asking || block !== null}
      testId={`call-${variant}-screen`}
      state={local.screen}
      onPress={() => {
        if (on) void video.setScreen(false)
        else if (picker) void video.setScreen(true)
        else void video.loadScreenSources()
      }}
    >
      <ScreenShareIcon stop={on} size={variant === 'tile' ? 15 : undefined} />
    </CallButton>
  )
}

export default function CallControls({ variant, call, actions, video }: CallControlsProps): React.JSX.Element {
  const busy = call.phase === 'joining'
  const locked = call.speakDenied ? micBlockedText('speak_denied', call.roomName) : call.serverMuted ? micBlockedText('server_muted', call.roomName) : null
  const micOff = call.muted || call.micDenied || call.deafened || locked !== null
  const size = variant === 'tile' ? 15 : undefined
  return (
    <div
      data-testid={`call-${variant}-controls`}
      role="group"
      aria-label="Call controls"
      className={`flex items-center ${variant === 'tile' ? 'gap-1.5' : 'gap-0.5'}`}
    >
      <CallButton
        variant={variant}
        label={locked ?? (call.micDenied ? MIC_DENIED_LINE : micOff ? 'Unmute' : 'Mute')}
        pressed={micOff}
        on={micOff}
        dead={busy || locked !== null}
        testId={`call-${variant}-mute`}
        onPress={() => actions.setMuted(!call.muted)}
      >
        {micOff ? <MicOffIcon size={size} /> : <MicIcon size={size} />}
      </CallButton>
      <CallButton
        variant={variant}
        label={call.deafened ? 'Undeafen' : 'Deafen'}
        pressed={call.deafened}
        on={call.deafened}
        dead={busy}
        testId={`call-${variant}-deafen`}
        onPress={() => actions.setDeafened(!call.deafened)}
      >
        <HeadphonesIcon off={call.deafened} size={size} />
      </CallButton>
      {video ? <CameraButton variant={variant} video={video} busy={busy} /> : null}
      {video ? <ScreenButton variant={variant} video={video} busy={busy} /> : null}
      <CallButton variant={variant} label="Leave call" danger testId={`call-${variant}-leave`} onPress={() => void actions.leave()}>
        <PhoneOffIcon size={size} />
      </CallButton>
    </div>
  )
}
