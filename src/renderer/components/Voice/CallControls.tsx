// The call's quick controls: Mute, Deafen, Noise removal (the wave; native
// engine only), Camera and Share screen (with
// video rooms on) and Hang up. They sit in the top bar beside the headset while the call has no
// video, and under the floating video tile while it has.
//
// Ported from K2 Companion's call tile strip (src/zen/voice/callTile.tsx,
// `Controls` and `TileButton`): the same buttons, labels and states (a
// moderator's mute shows locked and does nothing; a camera the room forbids
// shows dimmed). K2's look: the top bar's icon buttons, and in the tile's
// strip K2's elevated panel buttons. Every press goes to the voice and video
// stores, which drive the engine; nothing here holds call state.
//
// Outside the top bar the buttons follow the active look: round in Glass
// (`--radius-pill`) and cut in Terminal (`data-k2-chamfer="control"`), 36px
// in the headset list and the call view, 40px on the call's bar
// (CallControlBar: Frost in Glass, a raised cut bar in Terminal). Hang up
// stays the danger red. On the call's bar a muted mic also says Muted in
// words (MutedWord). Keyboard focus draws the shared ring (`k2-focus`
// outside Terminal, the cut ring in it), with a line of the label colour on
// a filled button so it reads on the fill too.

import type { ReactNode } from 'react'
import { useStore } from 'zustand'
import { Surface } from '@/components/ui/Surface'
import {
  HeadphonesIcon,
  MicIcon,
  MicOffIcon,
  NoiseWaveIcon,
  PhoneOffIcon,
  ScreenShareIcon,
  TOP_BAR_ICON_REST_CLASS,
  VideoIcon,
} from '@/components/icons/K2Icons'
import { micBlockedText, MIC_DENIED_LINE } from '@/lib/voice/voice-wire'
import { policyBlockText } from '@/lib/voice/video-labels'
import { recordingBadgeTitle, recordingButtons } from '@/lib/voice/recording-controls'
import type { VoiceCall } from '@/stores/voice-core'
import type { VideoController } from '@/stores/video-core'

/** `tile`: the floating tile's strip; `overlay`: the expanded view and the
 *  call's own window (the strip's buttons, their own test ids). */
export type CallControlsVariant = 'topbar' | 'tile' | 'overlay'

/** The floating tile strip's button (px). */
export const CALL_TILE_BUTTON = 28
/** The headset list's and the call view's buttons (px). */
export const CALL_PANEL_BUTTON = 36
/** The call bar's buttons (the expanded view and the call's own window, px). */
export const CALL_BAR_BUTTON = 40
/** The icon inside a headset-list or call-view button (px). */
export const CALL_PANEL_ICON = 17
/** The icon inside a call bar button (px). */
const CALL_BAR_ICON = 18
/** The icon inside a tile-strip button (px). */
const CALL_TILE_ICON = 15

function buttonSize(variant: CallControlsVariant): number {
  return variant === 'overlay' ? CALL_BAR_BUTTON : CALL_TILE_BUTTON
}

function iconSize(variant: CallControlsVariant): number | undefined {
  return variant === 'overlay' ? CALL_BAR_ICON : variant === 'tile' ? CALL_TILE_ICON : undefined
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const noDrag = { WebkitAppRegion: 'no-drag' } as any

const TOPBAR_ON = 'text-[var(--color-status-error)] hover:bg-[var(--color-bg-elevated)]'
const TOPBAR_DANGER = 'text-[var(--color-danger)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-danger-hover)]'
/** The look's control shape: round in Glass, square here and cut by
 *  `data-k2-chamfer="control"` in Terminal; the shared focus ring. */
const SHAPE = 'k2-focus rounded-[var(--radius-pill)]'
const TILE_BASE =
  'border border-[var(--color-control-edge)] bg-[var(--color-bg-elevated)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-control-hover)]'
/** Engaged (muted, deafened, the camera on): inverted, as Companion's white. */
const TILE_ON = 'border border-[var(--color-text-primary)] bg-[var(--color-text-primary)] text-[var(--color-bg)] [--k2-ch-focus-inner:var(--color-bg)]'
/** Hang up: the app's danger color (as the danger Button). */
const TILE_DANGER =
  'border border-[var(--color-danger)] bg-[var(--color-danger)] text-[var(--color-on-danger)] hover:bg-[var(--color-danger-active)] hover:border-[var(--color-danger-active)] [--k2-ch-focus-inner:var(--color-on-danger)]'

/** The bar the call's controls sit on (the expanded view, the call's own
 *  window): Frost in Glass, a pill floating over the call (the tiles stay
 *  solid); a raised bar with cut corners in Terminal. */
export function CallControlBar({ children, className = '' }: { children: ReactNode; className?: string }): React.JSX.Element {
  return (
    <Surface
      role2="elevated"
      chamfer
      data-testid="call-control-bar"
      data-material="frost"
      data-frost-shape="pill"
      data-frost-static=""
      className={`flex items-center px-2.5 py-2 ${className}`}
    >
      {children}
    </Surface>
  )
}

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
      data-k2-chamfer="control"
      className={`flex shrink-0 items-center justify-center transition-colors disabled:cursor-default disabled:opacity-50 ${SHAPE} ${look}`}
      style={{ width: CALL_PANEL_BUTTON, height: CALL_PANEL_BUTTON, ...noDrag }}
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
  badge = null,
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
  /** A word drawn on the button's lower edge (Muted), over the glyph's
   *  state; it never changes the button's size. */
  badge?: ReactNode
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
      data-k2-chamfer={variant !== 'topbar' ? 'control' : undefined}
      // The top bar's small icon buttons take the same ring with no cut:
      // square in Terminal (its radius tokens are 0), a circle in Glass.
      className={`relative flex shrink-0 items-center justify-center transition-colors ${SHAPE} ${look} ${dead ? 'opacity-50' : ''}`}
      style={variant !== 'topbar' ? { width: buttonSize(variant), height: buttonSize(variant), ...noDrag } : noDrag}
      onClick={() => {
        if (!dead) onPress()
      }}
    >
      {children}
      {badge}
    </button>
  )
}

/** "Muted" on the mic button's lower edge, on the call's bar: the state in
 *  words beside the mic-off glyph. Small (the badge type step) and in the
 *  look's chip (a pill in Glass, cut in Terminal), on the raised fill with
 *  the control edge so it reads on the engaged button and the bar alike.
 *  It hangs into the bar's padding, so nothing in the bar moves when it
 *  shows. The button's own name says it for screen readers. */
function MutedWord({ testId }: { testId: string }): React.JSX.Element {
  return (
    <span
      aria-hidden
      data-testid={testId}
      data-k2-chamfer="chip"
      className="type-badge pointer-events-none absolute -bottom-1.5 left-1/2 z-[2] -translate-x-1/2 whitespace-nowrap rounded-[var(--radius-pill)] border border-[var(--color-control-edge)] bg-[var(--color-bg-elevated)] px-1 leading-3 text-[var(--color-text-primary)]"
    >
      Muted
    </span>
  )
}

export interface CallControlsProps {
  variant: CallControlsVariant
  call: VoiceCall
  /** The voice controller's actions (the app's, or a test's). Record and
   *  Stop are there with the native engine (Sew 0.9.0). */
  actions: {
    setMuted(m: boolean): void
    setDeafened(d: boolean): void
    leave(): Promise<void>
    startRecording?(): Promise<void>
    stopRecording?(): Promise<void>
    /** Noise removal (Sew 0.10.2): the same choice as Settings → Voice →
     *  Noise removal, saved for this computer and sent to the live call. */
    setNoiseRemoval?(on: boolean): void
  }
  /** With video rooms on: the camera button. */
  video: VideoController | null
  /** The wave button's state (the voice prefs' `noiseRemoval`), with the
   *  native engine only; absent or null: no button (a seat-only call). */
  noiseRemoval?: boolean | null
}

/** "Noise removal on / off". On is the plain look (it's the default, nothing
 *  to warn about); off is engaged, like a muted mic, with the slash. */
export function noiseRemovalLabel(on: boolean): string {
  return on ? 'Noise removal on' : 'Noise removal off'
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
      <VideoIcon off={!on} size={iconSize(variant)} />
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
  if (!on && !picker && !(variant !== 'topbar' && caps.screen.list)) return null
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
      <ScreenShareIcon stop={on} size={iconSize(variant)} />
    </CallButton>
  )
}

/** The red "Recording" badge, the whole time a recording is on. */
export function RecordingBadge({ call, variant }: { call: VoiceCall; variant: CallControlsVariant }): React.JSX.Element | null {
  const r = call.recording
  if (!r) return null
  const starter = r.startedBy ? (call.participants.find((p) => p.memberId === r.startedBy)?.user ?? null) : null
  return (
    <span
      data-testid={`call-${variant}-recording`}
      role="status"
      title={recordingBadgeTitle(r, starter)}
      className="flex shrink-0 items-center gap-1 px-1 text-[10px] font-semibold uppercase tracking-wide text-[var(--color-status-error-text)]"
      style={noDrag}
    >
      <span aria-hidden className="inline-block h-2 w-2 rounded-[var(--radius-pill)] bg-[var(--color-status-error)]" />
      Recording
    </span>
  )
}


export default function CallControls({ variant, call, actions, video, noiseRemoval }: CallControlsProps): React.JSX.Element {
  const busy = call.phase === 'joining'
  const locked = call.speakDenied ? micBlockedText('speak_denied', call.roomName) : call.serverMuted ? micBlockedText('server_muted', call.roomName) : null
  const micOff = call.muted || call.micDenied || call.deafened || locked !== null
  const size = iconSize(variant)
  const rec = recordingButtons({
    perms: call.recordingPerms,
    recording: call.recording,
    controls: call.recordingControls,
  })
  return (
    <div
      data-testid={`call-${variant}-controls`}
      role="group"
      aria-label="Call controls"
      className={`flex items-center ${variant === 'overlay' ? 'gap-2' : variant === 'tile' ? 'gap-1.5' : 'gap-0.5'}`}
    >
      <CallButton
        variant={variant}
        label={locked ?? (call.micDenied ? MIC_DENIED_LINE : micOff ? 'Unmute' : 'Mute')}
        pressed={micOff}
        on={micOff}
        dead={busy || locked !== null}
        testId={`call-${variant}-mute`}
        onPress={() => actions.setMuted(!call.muted)}
        badge={variant === 'overlay' && micOff ? <MutedWord testId={`call-${variant}-muted`} /> : null}
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
      {typeof noiseRemoval === 'boolean' && actions.setNoiseRemoval ? (
        <CallButton
          variant={variant}
          label={noiseRemovalLabel(noiseRemoval)}
          pressed={noiseRemoval}
          on={!noiseRemoval}
          testId={`call-${variant}-noise`}
          state={noiseRemoval ? 'on' : 'off'}
          onPress={() => actions.setNoiseRemoval?.(!noiseRemoval)}
        >
          <NoiseWaveIcon off={!noiseRemoval} size={size} />
        </CallButton>
      ) : null}
      {video ? <CameraButton variant={variant} video={video} busy={busy} /> : null}
      {video ? <ScreenButton variant={variant} video={video} busy={busy} /> : null}
      {rec.canRecord && actions.startRecording ? (
        <CallButton variant={variant} label="Record" dead={busy} testId={`call-${variant}-record`} onPress={() => void actions.startRecording?.()}>
          <span aria-hidden className="inline-block h-2.5 w-2.5 rounded-[var(--radius-pill)] border-2 border-[var(--color-status-error)]" />
        </CallButton>
      ) : null}
      {rec.canStop && actions.stopRecording ? (
        <CallButton variant={variant} label="Stop recording" dead={busy} testId={`call-${variant}-stop-recording`} onPress={() => void actions.stopRecording?.()}>
          <span aria-hidden className="inline-block h-2.5 w-2.5 bg-[var(--color-status-error)]" />
        </CallButton>
      ) : null}
      <RecordingBadge call={call} variant={variant} />
      <CallButton variant={variant} label="Leave call" danger testId={`call-${variant}-leave`} onPress={() => void actions.leave()}>
        <PhoneOffIcon size={size} />
      </CallButton>
    </div>
  )
}
