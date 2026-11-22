// Settings → Cores → Voice Rooms (prd-k2-voice-rooms-v1 §3.3, §4.5; lane D).
//
// The window's server only: why it can or can't host (§4.5), the room count
// stepper with "Close now" (§3.4), one row per room (name, people now,
// rename), and the media mode (tunnel, or the direct fallback with its
// "this box's address is public" note, §6.4). Read-only for Members: the
// daemon says who may change rooms (`canManage`), the page never guesses a
// role. Plus this computer's own call preference (join muted).
//
// Sew 0.9.0: "Require recording when two or more people are in a room"
// shows while the daemon says K2's desktop answers recording notices
// (`recording.consentSheet`, on since 0.46.11; Rosson's answer 6). The line
// "Agents on this server may read the transcript" shows while an agent holds
// "Read call transcripts" (`recording.agentReaders`).
//
// Shown in the nav only with the window's server's `voice_rooms_preview` on.
// A server that can't host (not Linux, not Pro…) shows the daemon's line. It
// reads on open, after each change and on `voice_rooms_changed`; no polling.
// Never `fetchProjects()`.

import { useCallback, useEffect, useState } from 'react'
import { openUrl } from '@tauri-apps/plugin-opener'
import { BetaTag, Button, Callout, Checkbox, Input, Spinner, Toggle } from '@/components/ui'
import { primaryScope } from '@/kessel/server-scope'
import { fetchVoiceRooms, fetchVoiceStatus, renameVoiceRoom, setVoiceRecordingRequired, setVoiceRoomCount } from '@/lib/voice/voice-api'
import type { VoiceHosting, VoiceRoom, VoiceRoomsList, VoiceStatus } from '@/lib/voice/voice-wire'
import { onVoiceRoomsChanged } from '@/stores/session-events'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useVoice, voice } from '@/stores/voice'
import { useSettingsStore } from '@/stores/settings'
import {
  SETTING_FIELD_WIDTH,
  SETTING_LIST_ROW_HOVER,
  SETTING_ROW_HINT,
  SettingRow,
  SettingsGroup,
  SettingsLoadState,
  SettingSwitchRow,
} from '../controls/SettingControls'
import { ActionNote, readFailure, useActionNote } from './ActionNote'
import type { SettingEntry } from '../searchManifest'
import { PRODUCT_NAME } from '@/lib/brand'

export const VOICE_ROOMS_MANIFEST: SettingEntry[] = [
  {
    id: 'voice-rooms.count',
    section: 'voice-rooms',
    label: 'Voice rooms',
    description: 'How many voice rooms this server hosts, and their names',
    keywords: ['voice', 'rooms', 'call', 'headset', 'count', 'rename', 'sew'],
  },
  {
    id: 'voice-rooms.join-muted',
    section: 'voice-rooms',
    label: 'Join voice rooms muted',
    description: 'Start every call on this computer with the microphone off',
    keywords: ['voice', 'mute', 'microphone', 'mic', 'join'],
  },
]

const MAX_ROOMS = 10
const CONTENT_WIDTH = 'max-w-[52rem]'

type HostingView = { line: string; tone: 'info' | 'warn'; link?: 'plans' | 'chat' }

/** Why the server can't host right now, in plain words (§4.5). The daemon's
 *  own line is the fallback for a state this client doesn't know; its
 *  `detail` shows quietly under it. Null while it hosts normally. */
export function hostingView(h: VoiceHosting): HostingView | null {
  switch (h.state) {
    case 'ok':
      return null
    case 'not_linux':
      return { line: 'Voice rooms are hosted on Linux servers. To join one, sign in to a Linux server from the server list.', tone: 'info' }
    case 'preview_off':
      return { line: 'Voice rooms are a preview, and they’re off on this server.', tone: 'info' }
    case 'sew_off':
      return { line: `Voice rooms need chat and voice, which isn’t on for ${PRODUCT_NAME} sign-ins on this server.`, tone: 'warn', link: 'chat' }
    case 'voice_off':
    case 'sew_start_failed':
      return { line: 'Voice isn’t set up on this server yet.', tone: 'warn', link: 'chat' }
    case 'starting':
      return { line: 'Turning on voice rooms…', tone: 'info' }
    case 'not_pro':
      return { line: 'Voice rooms need the Pro plan.', tone: 'info', link: 'plans' }
    case 'count_zero':
      // The Rooms card's empty state says it, with the next step.
      return null
    case 'voice_lane_reconnecting':
    case 'voice_lane_down':
      return { line: `Voice is reconnecting to ${PRODUCT_NAME}’s network. Calls carry on as soon as it’s back.`, tone: 'warn' }
    default:
      return { line: h.line ?? h.detail ?? 'Voice rooms aren’t available right now.', tone: 'warn' }
  }
}

/** What was wrong with an answer K2 couldn't read, in words. */
function shapeLine(shape: string): string {
  if (/^the server's answer/.test(shape)) return `${shape.charAt(0).toUpperCase()}${shape.slice(1)}.`
  return `Part of the server’s answer wasn’t in a form ${PRODUCT_NAME} can read.`
}

function occupancy(r: VoiceRoom): string {
  if (r.closing) return r.closesAt !== null ? 'Closing now' : 'Closing once it’s empty'
  const n = r.occupants.length
  if (n === 0) return 'Empty'
  const people = `${n} ${n === 1 ? 'person' : 'people'} in it now`
  return r.limit !== null && n >= r.limit ? `${people} · Full` : people
}

export function VoiceRoomsSection(): React.JSX.Element {
  const [rooms, setRooms] = useState<VoiceRoomsList | null>(null)
  const [status, setStatus] = useState<VoiceStatus | null>(null)
  const [roomsError, setRoomsError] = useState<unknown>(null)
  const [statusError, setStatusError] = useState<unknown>(null)
  const [count, setCount] = useState<number | null>(null)
  const [closeNow, setCloseNow] = useState(false)
  const [busy, setBusy] = useState(false)
  /** What is saving now: `count`, `recording`, or a room's id (a rename). */
  const [saving, setSaving] = useState<string | null>(null)
  const [retrying, setRetrying] = useState(false)
  const [names, setNames] = useState<Record<string, string>>({})
  const joinMuted = useVoice((s) => s.prefs.joinMuted)
  const countNote = useActionNote()
  const recordingNote = useActionNote()
  const roomsNote = useActionNote()

  const load = useCallback(async () => {
    const scope = primaryScope()
    let list: VoiceRoomsList
    try {
      list = await fetchVoiceRooms(scope)
      setRooms(list)
      setRoomsError(null)
    } catch (err) {
      setRoomsError(err ?? new Error('unknown'))
      return
    }
    if (!list.canManage) return
    // Owner/Admin (§9.4; the route's floor too); a Member never asks.
    try {
      const st = await fetchVoiceStatus(scope)
      setStatus(st)
      setStatusError(null)
      setCount((c) => c ?? st.count)
    } catch (err) {
      setStatusError(err ?? new Error('unknown'))
    }
  }, [])

  const retry = useCallback(() => {
    setRetrying(true)
    void load().finally(() => setRetrying(false))
  }, [load])

  useEffect(() => {
    void load()
    return onVoiceRoomsChanged(primaryScope(), () => void load())
  }, [load])

  // A server that can't host has no rooms to change, whoever is asking.
  const manage = rooms?.canManage === true && rooms.hosting.state !== 'not_linux'
  const hosting = status?.hosting ?? rooms?.hosting ?? null
  const why = hosting ? hostingView(hosting) : null
  const current = status?.count ?? null
  const lowering = current !== null && count !== null && count < current
  const dropped = lowering && rooms ? rooms.rooms.slice(count ?? 0) : []
  const occupiedDropped = dropped.some((r) => r.occupants.length > 0)
  const limit = rooms?.rooms.find((r) => r.limit !== null)?.limit ?? null

  const applyCount = async (): Promise<void> => {
    if (count === null || count === current) return
    const now = lowering && occupiedDropped && closeNow
    if (now) {
      const n = (current ?? 0) - count
      const yes = await useConfirmDialogStore.getState().confirm({
        title: n === 1 ? 'Close this room now?' : `Close ${n} rooms now?`,
        message:
          n === 1
            ? 'Everyone in it gets a 10-second notice, then is disconnected. The other rooms aren’t affected.'
            : 'Everyone in them gets a 10-second notice, then is disconnected. The other rooms aren’t affected.',
        confirmLabel: 'Close now',
        destructive: true,
      })
      if (!yes) return
    }
    setBusy(true)
    setSaving('count')
    countNote.clear()
    try {
      await setVoiceRoomCount(primaryScope(), count, now)
      setCloseNow(false)
      countNote.done(
        lowering
          ? now
            ? `Saved. ${count === 1 ? 'One room stays' : `${count} rooms stay`}; the others close after a 10-second notice.`
            : `Saved. ${count === 1 ? 'One room stays' : `${count} rooms stay`}; the others close once they’re empty.`
          : `Saved. This server now has ${count} ${count === 1 ? 'room' : 'rooms'}.`,
      )
      setCount(null)
      await load()
      void voice.refreshRooms()
    } catch (err) {
      countNote.failed('Couldn’t change the number of rooms. Nothing changed; try again.', err)
    } finally {
      setSaving(null)
      setBusy(false)
    }
  }

  const requireRecording = async (on: boolean): Promise<void> => {
    setBusy(true)
    setSaving('recording')
    recordingNote.clear()
    try {
      await setVoiceRecordingRequired(primaryScope(), on)
      recordingNote.done(on ? 'On. Calls with two or more people are recorded.' : 'Off. Calls aren’t recorded unless someone starts it.')
      await load()
    } catch (err) {
      recordingNote.failed(on ? 'Couldn’t turn this on. Calls still aren’t required to record.' : 'Couldn’t turn this off. Calls are still recorded.', err)
    } finally {
      setSaving(null)
      setBusy(false)
    }
  }

  const recording = status?.recording
  const showRequire = manage && recording?.consentSheet === true && recording.canRequire
  const agentReaders = recording?.agentReaders === true

  const rename = async (r: VoiceRoom): Promise<void> => {
    const name = (names[r.id] ?? '').trim()
    if (!name) return
    setBusy(true)
    setSaving(r.id)
    roomsNote.clear()
    try {
      await renameVoiceRoom(primaryScope(), r.id, name)
      setNames((n) => {
        const next = { ...n }
        delete next[r.id]
        return next
      })
      roomsNote.done(`Renamed “${r.name}” to “${name}”.`)
      await load()
      void voice.refreshRooms()
    } catch (err) {
      roomsNote.failed(`Couldn’t rename “${r.name}”. It keeps its name; try again.`, err)
    } finally {
      setSaving(null)
      setBusy(false)
    }
  }

  const tryAgain = (
    <Button disabled={retrying} onClick={retry}>
      {retrying ? 'Trying…' : 'Try again'}
    </Button>
  )

  const roomsFail = roomsError && !rooms ? readFailure(roomsError) : null
  const roomsState = !rooms ? (
    roomsFail ? (
      <SettingsLoadState kind="error" title="Couldn’t load the voice rooms" detail={roomsFail.detail} action={tryAgain} testId="voice-rooms-error">
        {PRODUCT_NAME} couldn&apos;t read this server&apos;s rooms.
        {roomsFail.shape ? <span className="block mt-1 text-[var(--color-text-muted)]">{shapeLine(roomsFail.shape)}</span> : null}
      </SettingsLoadState>
    ) : (
      <SettingsLoadState kind="loading" title="Loading the voice rooms…" testId="voice-rooms-loading" />
    )
  ) : rooms.rooms.length === 0 ? (
    <SettingsLoadState kind="empty" title="No rooms yet" testId="voice-rooms-empty">
      {manage ? 'Raise the number of rooms above to add one.' : 'The server’s owner or an Admin can add some.'}
    </SettingsLoadState>
  ) : undefined

  const statusFail = statusError && !status ? readFailure(statusError) : null
  const hostingState = !status ? (
    statusFail ? (
      <SettingsLoadState kind="error" title="Couldn’t load the room settings" detail={statusFail.detail} action={tryAgain} testId="voice-rooms-status-error">
        {PRODUCT_NAME} couldn&apos;t read how many rooms this server hosts.
        {statusFail.shape ? <span className="block mt-1 text-[var(--color-text-muted)]">{shapeLine(statusFail.shape)}</span> : null}
      </SettingsLoadState>
    ) : (
      <SettingsLoadState kind="loading" title="Loading the room settings…" />
    )
  ) : undefined

  // A server that can't host and has no rooms: the line above says why, and
  // there is nothing to list.
  const showRooms = !rooms || rooms.rooms.length > 0 || manage || why === null

  return (
    <div data-testid="voice-rooms-section" className={`w-full ${CONTENT_WIDTH} space-y-6 pb-6`}>
      <header className="min-w-0">
        <h2 className="type-title text-[var(--color-text-primary)] flex items-center gap-2">
          Voice Rooms
          <BetaTag />
        </h2>
        <p className={SETTING_ROW_HINT}>
          Voice rooms on this server, for its owner, Admins and Members. People join them from the headset in the top bar.
        </p>
      </header>

      {why && hosting ? (
        <div data-testid="voice-rooms-hosting" data-state={hosting.state}>
          <SettingsLoadState
            kind="empty"
            title={why.line}
            detail={hosting.detail}
            action={
              why.link === 'plans' ? (
                <Button onClick={() => void openUrl('https://k2.dev/dashboard').catch(() => roomsNote.failed('Couldn’t open the plans page.', null))}>
                  See plans
                </Button>
              ) : why.link === 'chat' ? (
                <Button onClick={() => useSettingsStore.getState().setSection('sew')}>Open Chat / Voice</Button>
              ) : undefined
            }
          />
        </div>
      ) : null}

      {manage ? (
        <SettingsGroup title="Hosting" card settingId="voice-rooms.count" testId="voice-rooms-hosting-group" state={hostingState}>
          <SettingRow
            label="Number of rooms"
            hint={`Up to ${MAX_ROOMS}${limit !== null ? `, each for up to ${limit} people` : ''}.`}
            testId="voice-rooms-count-row"
            below={
              lowering || countNote.note ? (
                <div className="space-y-2">
                  {lowering ? (
                    <p className={SETTING_ROW_HINT}>
                      {dropped.length === 1 ? 'The last room closes' : `The last ${dropped.length} rooms close`} once empty; anyone in them stays until
                      they leave.
                    </p>
                  ) : null}
                  {lowering && occupiedDropped ? (
                    <label className="flex items-center gap-2 type-row text-[var(--color-text-secondary)]">
                      <Checkbox data-testid="voice-rooms-close-now" checked={closeNow} onChange={(e) => setCloseNow(e.currentTarget.checked)} />
                      {dropped.length === 1
                        ? 'Close it now: everyone in it gets a 10-second notice, then is disconnected'
                        : 'Close them now: everyone in them gets a 10-second notice, then is disconnected'}
                    </label>
                  ) : null}
                  <ActionNote note={countNote.note} testId="voice-rooms-count-note" />
                </div>
              ) : undefined
            }
          >
            <div className="flex items-center gap-1" role="group" aria-label="Number of rooms">
              <Button
                variant="quiet"
                aria-label="One room fewer"
                data-testid="voice-rooms-minus"
                disabled={busy || count === null || count <= 0}
                onClick={() => {
                  countNote.clear()
                  setCount((c) => (c === null ? c : Math.max(0, c - 1)))
                }}
              >
                −
              </Button>
              <span data-testid="voice-rooms-count" aria-live="polite" className="w-8 text-center type-row tabular-nums text-[var(--color-text-primary)]">
                {count ?? '–'}
              </span>
              <Button
                variant="quiet"
                aria-label="One more room"
                data-testid="voice-rooms-plus"
                disabled={busy || count === null || count >= MAX_ROOMS}
                onClick={() => {
                  countNote.clear()
                  setCount((c) => (c === null ? c : Math.min(MAX_ROOMS, c + 1)))
                }}
              >
                +
              </Button>
            </div>
            <Button variant="accent" data-testid="voice-rooms-apply" aria-busy={saving === 'count' || undefined} disabled={busy || count === null || count === current} onClick={() => void applyCount()}>
              {saving === 'count' ? 'Saving…' : 'Save'}
            </Button>
          </SettingRow>
          {showRequire && recording ? (
            <SettingSwitchRow
              testId="voice-rooms-require-row"
              control={
                <>
                {saving === 'recording' ? (
                  <span role="status" className="inline-flex items-center gap-1.5 type-hint text-[var(--color-text-secondary)] whitespace-nowrap">
                    <Spinner />
                    Saving…
                  </span>
                ) : null}
                <Toggle
                  data-testid="voice-rooms-require-recording"
                  aria-label="Require recording when two or more people are in a room"
                  checked={recording.required === true}
                  disabled={busy || recording.locked}
                  onChange={(on) => void requireRecording(on)}
                />
                </>
              }
            >
              <div className="type-row text-[var(--color-text-primary)]">Require recording when two or more people are in a room</div>
              <div className={SETTING_ROW_HINT}>
                Everyone in the call is told, and either agrees or leaves.
                {recording.locked ? ' Your organisation sets this, so it can’t be changed here.' : ''}
              </div>
              {agentReaders ? (
                <div className={SETTING_ROW_HINT} data-testid="voice-rooms-agent-readers">
                  Agents on this server may read the transcript. Each person&apos;s recording notice says so. Change it per agent in
                  Workspaces / Agents, under Read call transcripts.
                </div>
              ) : null}
              {recordingNote.note ? (
                <div className="mt-2">
                  <ActionNote note={recordingNote.note} testId="voice-rooms-recording-note" />
                </div>
              ) : null}
            </SettingSwitchRow>
          ) : null}
          {status?.mediaTier ? (
            <SettingRow
              label="How calls connect"
              hint={
                <span data-testid="voice-rooms-media">
                  {status.mediaTier === 'tunnel'
                    ? `Through ${PRODUCT_NAME}’s tunnel, so this server’s address isn’t published.`
                    : 'Directly. This server’s public address is in DNS and in every join answer, so people in a call can see it.'}
                </span>
              }
            >
              <span className="type-row text-[var(--color-text-secondary)]">{status.mediaTier === 'tunnel' ? `Through ${PRODUCT_NAME}` : 'Direct'}</span>
            </SettingRow>
          ) : null}
        </SettingsGroup>
      ) : null}

      {!showRequire && agentReaders ? (
        <p className={SETTING_ROW_HINT} data-testid="voice-rooms-agent-readers">
          Agents on this server may read the transcript. Each person&apos;s recording notice says so.
        </p>
      ) : null}

      {showRooms ? (
        <SettingsGroup
          title="Rooms"
          hint={manage ? 'Rename a room here; everyone sees the new name in the headset list.' : undefined}
          card
          testId="voice-rooms-list"
          state={roomsState}
        >
          {(rooms?.rooms ?? []).map((r, i) => {
            const draft = names[r.id]
            const changed = draft !== undefined && draft.trim() !== '' && draft.trim() !== r.name
            return (
              <div key={r.id} data-testid="voice-rooms-row" className={SETTING_LIST_ROW_HOVER}>
                <SettingRow label={manage ? `Room ${i + 1}` : <span className="type-name">{r.name}</span>} hint={occupancy(r)}>
                  {manage ? (
                    <form
                      className="flex items-center gap-2"
                      onSubmit={(e) => {
                        e.preventDefault()
                        if (changed) void rename(r)
                      }}
                    >
                      <Input
                        aria-label={`Name of ${r.name}`}
                        className={SETTING_FIELD_WIDTH}
                        value={draft ?? r.name}
                        maxLength={64}
                        onChange={(e) => {
                          const v = e.currentTarget.value
                          roomsNote.clear()
                          setNames((n) => ({ ...n, [r.id]: v }))
                        }}
                      />
                      <Button type="submit" aria-busy={saving === r.id || undefined} disabled={busy || !changed}>
                        {saving === r.id ? 'Renaming…' : 'Rename'}
                      </Button>
                    </form>
                  ) : null}
                </SettingRow>
              </div>
            )
          })}
          {roomsNote.note ? (
            <div className="px-4 py-3">
              <ActionNote note={roomsNote.note} testId="voice-rooms-note" />
            </div>
          ) : null}
        </SettingsGroup>
      ) : null}

      {rooms && roomsError ? (
        <Callout tone="warn" role="alert" className="type-hint flex items-center justify-between gap-4" data-testid="voice-rooms-refresh-error">
          <span>Couldn&apos;t refresh the rooms, so this list may be out of date.</span>
          {tryAgain}
        </Callout>
      ) : null}

      <SettingsGroup title="On this computer" card settingId="voice-rooms.join-muted">
        <SettingSwitchRow
          control={
            <Toggle
              data-testid="voice-join-muted"
              aria-label="Join with my microphone off"
              checked={joinMuted}
              onChange={(on) => voice.setPrefs({ joinMuted: on })}
            />
          }
        >
          <div className="type-row text-[var(--color-text-primary)]">Join with my microphone off</div>
          <div className={SETTING_ROW_HINT}>Every call you join from this computer starts muted. Unmute any time in the call.</div>
        </SettingSwitchRow>
      </SettingsGroup>
    </div>
  )
}
