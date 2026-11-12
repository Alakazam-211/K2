// Voice rooms §4.3–§4.6: the headset's list. Top: the current call (room,
// server, people, time, Mute / Deafen / Leave, devices). Then one section
// per signed-in server that reports rooms, "This server" first, then A to Z
// (S5). Every action goes to THAT server with its own login.

import { useEffect, useState } from 'react'
import { floatMaterial } from '@/components/ui/float-material'
import PresenceAvatar from '@/components/Presence/PresenceAvatar'
import type { PresenceRole } from '@/components/Presence/PresenceChip'
import { roomFailureActions } from '@/components/Home/room/RoomFailure'
import { SettingDropdown } from '@/components/Settings/controls/SettingControls'
import { useSettingsStore } from '@/stores/settings'
import { useVoice, voice } from '@/stores/voice'
import type { VoiceCall, ServerRooms, WindowHosting } from '@/stores/voice-core'
import type { VoiceDevice } from '@/lib/voice/voice-engine'
import { MIC_DENIED_LINE, hostingStateText, isLaneReconnecting, micBlockedText, type VoiceOccupant, type VoiceRoom } from '@/lib/voice/voice-wire'
import { lastSeenText, type VoiceServer } from '@/lib/voice/voice-servers'
import { isWebClient } from '@/lib/is-web'
import { callDuration, roomStateText } from './voice-labels'
import { VOICE_CHAT_LINE } from '@/lib/voice-chat-only'
import { VoiceChatOnlyChip } from './VoiceChatOnlyChip'

/** While a join runs, another Join (and "Leave and join") would leave it
 *  half way, so they wait. */
const JOINING_WAIT = 'Joining… wait a moment'
import { video } from '@/stores/video'
import { CameraPicker, ScreenPicker, VideoCallButtons, VideoNoticeLine } from './VideoControls'
import { CALL_PANEL_ICON, CallPanelButton } from './CallControls'
import { HeadphonesIcon, MicIcon, MicOffIcon, PhoneOffIcon } from '@/components/icons/K2Icons'

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const noDrag = { WebkitAppRegion: 'no-drag' } as any

const SMALL_BTN =
  'border border-[var(--color-border)] px-2 py-0.5 text-[11px] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)] disabled:opacity-50'

function roleOf(r: string | null): PresenceRole {
  return r === 'owner' || r === 'admin' ? r : 'member'
}

function Occupants({ list, size }: { list: VoiceOccupant[]; size: number }): React.JSX.Element {
  return (
    <span className="flex flex-wrap items-center" style={{ gap: 3 }}>
      {list.map((o) => (
        <span
          key={o.user}
          data-testid="voice-occupant"
          data-speaking={o.speaking ? 'true' : 'false'}
          className={o.speaking ? 'outline outline-2 outline-[var(--color-accent)]' : ''}
          title={`${o.name ?? o.user}${o.muted ? ' (muted)' : ''}`}
        >
          <PresenceAvatar name={o.user} role={roleOf(o.role)} size={size} color={o.color} />
        </span>
      ))}
    </span>
  )
}

/** "System default" ('' → null) first, then the devices in the engine's order. */
function deviceOptions(list: VoiceDevice[]): { value: string; label: string }[] {
  return [{ value: '', label: 'System default' }, ...list.map((d) => ({ value: d.id, label: d.label }))]
}

function CallPanel({ call }: { call: VoiceCall }): React.JSX.Element {
  const devices = useVoice((s) => s.devices)
  const prefs = useVoice((s) => s.prefs)
  const engineKind = useVoice((s) => s.engineKind)
  // Video (preview, off by default): the camera and screen controls.
  const videoOn = useSettingsStore((s) => s.videoRoomsPreview) === true && engineKind === 'native'
  useEffect(() => {
    if (call.phase !== 'connected') return
    void voice.loadDevices().catch((err: unknown) => console.warn('[voice] reading audio devices failed:', err))
  }, [call.phase])
  const phaseLine =
    call.phase === 'joining'
      ? 'Joining…'
      : call.phase === 'connecting'
        ? 'Connecting…'
        : call.phase === 'reconnecting'
          ? 'Reconnecting…'
          : callDuration(call.connectedAt, Date.now())
  const busy = call.phase === 'joining'
  const micBlocked = call.speakDenied ? micBlockedText('speak_denied', call.roomName) : call.serverMuted ? micBlockedText('server_muted', call.roomName) : null
  // Deafen mutes too (the store sets `muted`), so this covers it.
  const micOff = call.muted || call.micDenied || micBlocked !== null
  return (
    <section data-testid="voice-call" data-phase={call.phase} className="border-b border-[var(--color-border)] -mx-3 px-3 pb-2 mb-2">
      <div className="flex items-center justify-between">
        <span className="text-[12px] text-[var(--color-text-primary)]">
          {call.roomName} <span className="text-[var(--color-text-muted)]">· {call.serverLabel}</span>
        </span>
        <span data-testid="voice-call-phase" className="text-[11px] font-mono text-[var(--color-text-muted)]">
          {phaseLine}
        </span>
      </div>
      {call.relayOnly ? (
        <p data-testid="voice-relay-only" className="mt-0.5 text-[10px] text-[var(--color-text-muted)]">
          Through K2's network (relay only)
        </p>
      ) : null}
      {engineKind === 'seat' ? (
        <p data-testid="voice-seat-only" className="mt-0.5 text-[10px] text-[var(--color-text-muted)]">
          {isWebClient() ? 'You can see this room here, but not talk. Use the K2 desktop app to talk.' : 'No call audio in this version: you hold a seat in the room.'}
        </p>
      ) : null}
      {call.micDenied ? (
        <p data-testid="voice-mic-denied" className="mt-1 text-[11px] text-[var(--color-status-error-soft)]">
          {MIC_DENIED_LINE}{' '}
          <button type="button" data-testid="voice-mic-settings" className="underline hover:text-[var(--color-text-primary)]" onClick={() => void voice.openMicSettings()}>
            Open privacy settings
          </button>
        </p>
      ) : null}
      {call.participants.length > 0 ? (
        <div className="mt-1">
          <Occupants
            size={20}
            list={call.participants.map((p) => ({ user: p.user, name: null, role: null, color: null, muted: p.muted, deafened: false, speaking: p.speaking }))}
          />
        </div>
      ) : null}
      <div data-testid="voice-call-controls" role="group" aria-label="Call controls" className="mt-2 flex items-center gap-1.5">
        <CallPanelButton
          testId="voice-mute"
          label={micOff ? 'Unmute' : 'Mute'}
          title={micBlocked ?? undefined}
          on={micOff}
          disabled={busy || micBlocked !== null}
          onPress={() => voice.setMuted(!call.muted)}
        >
          {micOff ? <MicOffIcon size={CALL_PANEL_ICON} /> : <MicIcon size={CALL_PANEL_ICON} />}
        </CallPanelButton>
        <CallPanelButton
          testId="voice-deafen"
          label={call.deafened ? 'Undeafen' : 'Deafen'}
          on={call.deafened}
          disabled={busy}
          onPress={() => voice.setDeafened(!call.deafened)}
        >
          <HeadphonesIcon off={call.deafened} size={CALL_PANEL_ICON} />
        </CallPanelButton>
        {videoOn ? <VideoCallButtons ctl={video} disabled={busy} /> : null}
        <CallPanelButton testId="voice-leave" label="Leave call" danger onPress={() => void voice.leave()}>
          <PhoneOffIcon size={CALL_PANEL_ICON} />
        </CallPanelButton>
      </div>
      {videoOn ? <ScreenPicker ctl={video} /> : null}
      {videoOn ? <VideoNoticeLine ctl={video} /> : null}
      {devices && (devices.inputs.length > 0 || devices.outputs.length > 0) ? (
        <div className="mt-2 flex flex-col gap-1 text-[11px] text-[var(--color-text-secondary)]">
          <div className="flex items-center justify-between gap-2">
            Microphone
            <SettingDropdown
              compact
              className="min-w-0 max-w-[170px]"
              ariaLabel="Microphone"
              triggerData={{ 'data-testid': 'voice-input' }}
              value={prefs.inputDeviceId ?? ''}
              options={deviceOptions(devices.inputs)}
              onChange={(v) => voice.setPrefs({ inputDeviceId: v || null })}
            />
          </div>
          <div className="flex items-center justify-between gap-2">
            Speakers
            <SettingDropdown
              compact
              className="min-w-0 max-w-[170px]"
              ariaLabel="Speakers"
              triggerData={{ 'data-testid': 'voice-output' }}
              disabled={!devices.outputSelectable}
              title={devices.outputSelectable ? undefined : 'This system plays calls on its default output.'}
              value={prefs.outputDeviceId ?? ''}
              options={deviceOptions(devices.outputs)}
              onChange={(v) => voice.setPrefs({ outputDeviceId: v || null })}
            />
          </div>
          {videoOn ? <CameraPicker ctl={video} /> : null}
        </div>
      ) : null}
    </section>
  )
}

function RoomRow({
  server,
  room,
  call,
  onJoin,
}: {
  server: VoiceServer
  room: VoiceRoom
  call: VoiceCall | null
  onJoin: (room: VoiceRoom) => void
}): React.JSX.Element {
  const here = call !== null && call.hostKey === server.hostKey && call.roomId === room.id
  const joining = call?.phase === 'joining'
  const state = roomStateText(room, Date.now())
  return (
    <li data-testid="voice-room" data-room={room.id} className="flex items-center gap-2 py-1">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-1 text-[12px] text-[var(--color-text-primary)]">
          <span className="truncate">{room.name}</span>
          <span data-testid="voice-room-count" className="text-[10px] font-mono text-[var(--color-text-muted)]">
            {room.occupants.length}
            {room.limit !== null ? `/${room.limit}` : ''}
          </span>
        </div>
        {room.occupants.length > 0 ? (
          <div className="mt-0.5">
            <Occupants list={room.occupants} size={14} />
          </div>
        ) : null}
      </div>
      {here ? (
        <span className="text-[11px] text-[var(--color-accent)]">You're here</span>
      ) : state === null ? (
        <button
          type="button"
          data-testid="voice-join"
          className={SMALL_BTN}
          disabled={joining}
          title={joining ? JOINING_WAIT : undefined}
          onClick={() => onJoin(room)}
        >
          Join
        </button>
      ) : (
        <span data-testid="voice-room-state" className="text-[11px] text-[var(--color-text-muted)]" title={room.unavailable ?? undefined}>
          {state}
        </span>
      )}
    </li>
  )
}

function ServerSection({
  server,
  rooms,
  call,
  onJoin,
}: {
  server: VoiceServer
  rooms: ServerRooms | undefined
  call: VoiceCall | null
  onJoin: (server: VoiceServer, room: VoiceRoom) => void
}): React.JSX.Element {
  const title = server.isWindow ? `This server · ${server.label}` : server.label
  if (server.kind === 'offline') {
    return (
      <section data-testid="voice-server" data-kind="offline" className="py-1 opacity-60">
        <header className="text-[11px] uppercase tracking-wide text-[var(--color-text-muted)]">{title}</header>
        <p className="text-[11px] text-[var(--color-text-muted)]">{lastSeenText(server.lastSeenAt, Date.now())}</p>
      </section>
    )
  }
  if (server.kind === 'signed-out') {
    return (
      <section data-testid="voice-server" data-kind="signed-out" className="py-1 opacity-70">
        <header className="text-[11px] uppercase tracking-wide text-[var(--color-text-muted)]">{title}</header>
        <div className="flex items-center gap-2">
          <p className="flex-1 text-[11px] text-[var(--color-text-muted)]">
            {server.authNote ?? `Sign in to ${server.label} to see its rooms.`}
          </p>
          <button type="button" data-testid="voice-sign-in" className={SMALL_BTN} onClick={() => roomFailureActions(server.hostKey).signIn()}>
            Sign in
          </button>
        </div>
      </section>
    )
  }
  const list = rooms?.list ?? null
  const hostingLine = list ? hostingStateText(list.hosting) : null
  const reconnecting = list ? isLaneReconnecting(list.hosting) : false
  return (
    <section data-testid="voice-server" data-kind="live" data-host={server.hostKey} className="py-1">
      <header className="flex items-center gap-1.5 text-[11px] uppercase tracking-wide text-[var(--color-text-muted)]">
        <span className="truncate">{title}</span>
        {server.voiceChatOnly ? <VoiceChatOnlyChip /> : null}
      </header>
      {server.voiceChatOnly ? (
        <p data-testid="voice-chat-only-line" className="text-[11px] text-[var(--color-text-muted)]">
          {VOICE_CHAT_LINE}
        </p>
      ) : null}
      {rooms?.error && !list ? (
        <p data-testid="voice-server-error" className="text-[11px] text-[var(--color-status-error-soft)]">
          Couldn't read {server.label}'s rooms. {rooms.error}
        </p>
      ) : null}
      {!list && !rooms?.error ? <p className="text-[11px] text-[var(--color-text-muted)]">Loading…</p> : null}
      {reconnecting ? (
        <p data-testid="voice-reconnecting" className="text-[11px] text-[var(--color-text-secondary)]">
          {hostingLine}
        </p>
      ) : null}
      {list && list.rooms.length > 0 ? (
        <ul>
          {list.rooms.map((r) => (
            <RoomRow key={r.id} server={server} room={r} call={call} onJoin={(room) => onJoin(server, room)} />
          ))}
        </ul>
      ) : null}
      {list && hostingLine && !reconnecting && (list.canManage || list.rooms.length > 0) ? (
        <p data-testid="voice-hosting" className="mt-1 text-[11px] text-[var(--color-text-secondary)]">
          {hostingLine}
          {server.isWindow && list.canManage ? (
            <>
              {' '}
              <button
                type="button"
                data-testid="voice-open-settings"
                className="underline hover:text-[var(--color-text-primary)]"
                onClick={() => useSettingsStore.getState().setSection('voice-rooms')}
              >
                Settings
              </button>
            </>
          ) : null}
        </p>
      ) : null}
    </section>
  )
}

/** The dimmed headset's list (no server hosts rooms, the window's server has
 *  the preview on): why this server doesn't host, in the daemon's own words
 *  (`hosting.line`, the line Settings → Voice Rooms shows). */
function WindowHostingSection({ wh }: { wh: WindowHosting | null }): React.JSX.Element {
  const list = wh?.list ?? null
  const line = list ? (hostingStateText(list.hosting) ?? list.hosting.line) : null
  return (
    <section data-testid="voice-window-hosting" data-state={list?.hosting.state} className="py-1">
      <header className="text-[11px] uppercase tracking-wide text-[var(--color-text-muted)]">
        {wh ? `This server · ${wh.label}` : 'This server'}
      </header>
      {!list && !wh?.error ? <p className="text-[11px] text-[var(--color-text-muted)]">Loading…</p> : null}
      {wh?.error && !list ? (
        <p data-testid="voice-window-hosting-error" className="text-[11px] text-[var(--color-status-error-soft)]">
          Couldn't read why this server doesn't host voice rooms. {wh.error}
        </p>
      ) : null}
      {line ? (
        <p data-testid="voice-window-hosting-line" className="text-[11px] text-[var(--color-text-secondary)]">
          {line}
          {list?.canManage ? (
            <>
              {' '}
              <button
                type="button"
                data-testid="voice-open-settings"
                className="underline hover:text-[var(--color-text-primary)]"
                onClick={() => useSettingsStore.getState().setSection('voice-rooms')}
              >
                Settings
              </button>
            </>
          ) : null}
        </p>
      ) : null}
      {list?.hosting.detail ? (
        <p data-testid="voice-window-hosting-detail" className="mt-0.5 text-[10px] text-[var(--color-text-muted)]">
          {list.hosting.detail}
        </p>
      ) : null}
    </section>
  )
}

export default function VoicePopover({
  menuRef,
  style,
  unavailable = false,
  onVideoDemo,
}: {
  /** Placed by the headset's anchored menu (VoiceButton), which also
   *  claims its box over a live Browser page while it is open. */
  menuRef?: React.RefCallback<HTMLDivElement>
  style?: React.CSSProperties
  unavailable?: boolean
  /** Dev builds with the video preview on: open the fake-engine video demo. */
  onVideoDemo?: () => void
}): React.JSX.Element {
  const servers = useVoice((s) => s.servers)
  const rooms = useVoice((s) => s.rooms)
  const call = useVoice((s) => s.call)
  const notice = useVoice((s) => s.notice)
  const windowHosting = useVoice((s) => s.windowHosting)
  const [confirm, setConfirm] = useState<{ server: VoiceServer; room: VoiceRoom } | null>(null)

  const join = (server: VoiceServer, room: VoiceRoom): void => {
    // §4.3: joining while in a call leaves the current call first, after a confirm.
    if (call) {
      setConfirm({ server, room })
      return
    }
    void voice.join(server.hostKey, room)
  }

  const live = servers.filter((s) => s.kind === 'live')
  const anyRooms = live.some((s) => (rooms[s.hostKey]?.list?.rooms.length ?? 0) > 0)
  const allRead = live.every((s) => rooms[s.hostKey]?.list || rooms[s.hostKey]?.error)

  return (
    <div
      ref={menuRef}
      role="dialog"
      aria-label="Voice rooms"
      data-testid="voice-popover"
      {...floatMaterial('popover')} className="w-[320px] max-h-[calc(var(--k2-vh)*0.7)] overflow-y-auto border border-[var(--color-border)] bg-[var(--color-bg)] px-3 py-2 shadow-lg"
      style={{ ...style, ...noDrag }}
    >
      <header className="mb-1 text-[12px] text-[var(--color-text-primary)]">Voice rooms</header>
      {call ? <CallPanel call={call} /> : null}
      {notice ? (
        <p data-testid="voice-notice" data-kind={notice.kind} className={`mb-1 text-[11px] ${notice.kind === 'error' ? 'text-[var(--color-status-error-soft)]' : 'text-[var(--color-text-secondary)]'}`}>
          {notice.text}
          {notice.micSettings && !call?.micDenied ? (
            <>
              {' '}
              <button type="button" data-testid="voice-mic-settings" className="underline hover:text-[var(--color-text-primary)]" onClick={() => void voice.openMicSettings()}>
                Open privacy settings
              </button>
            </>
          ) : null}
        </p>
      ) : null}
      {confirm ? (
        <div data-testid="voice-confirm" className="mb-2 border border-[var(--color-border)] p-2 text-[11px] text-[var(--color-text-secondary)]">
          <p>
            Leave {call?.roomName ?? 'this call'} and join {confirm.room.name}?
          </p>
          <div className="mt-1 flex gap-1">
            <button
              type="button"
              data-testid="voice-confirm-join"
              className={SMALL_BTN}
              disabled={call?.phase === 'joining'}
              title={call?.phase === 'joining' ? JOINING_WAIT : undefined}
              onClick={() => {
                const c = confirm
                setConfirm(null)
                void voice.join(c.server.hostKey, c.room)
              }}
            >
              Leave and join
            </button>
            <button type="button" className={SMALL_BTN} onClick={() => setConfirm(null)}>
              Cancel
            </button>
          </div>
        </div>
      ) : null}
      {unavailable ? <WindowHostingSection wh={windowHosting} /> : null}
      {servers.map((s) => (
        <ServerSection key={s.hostKey} server={s} rooms={rooms[s.hostKey]} call={call} onJoin={join} />
      ))}
      {!anyRooms && allRead && !unavailable ? (
        <p data-testid="voice-empty" className="mt-1 text-[11px] text-[var(--color-text-muted)]">
          No voice rooms yet. Rooms show here from Linux servers with Pro that you're signed in to.
        </p>
      ) : null}
      {onVideoDemo ? (
        <button type="button" data-testid="video-demo-open" className="mt-1 text-[10px] text-[var(--color-text-muted)] underline" onClick={onVideoDemo}>
          Video demo (dev)
        </button>
      ) : null}
    </div>
  )
}
