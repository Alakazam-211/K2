// Settings → Cores → Voice Rooms (prd-k2-voice-rooms-v1 §3.3, §4.5; lane D).
//
// The window's server only: why it can or can't host (§4.5), the room count
// stepper with "Close now" (§3.4), one row per room (name, people now,
// rename), and the media mode (tunnel, or the direct fallback with its
// "this box's address is public" note, §6.4). Read-only for Members: the
// daemon says who may change rooms (`canManage`), the page never guesses a
// role. Plus this computer's own call preference (join muted).
//
// Shown in the nav only with the window's server's `voice_rooms_preview` on.
// A server that can't host (not Linux, not Pro…) shows the daemon's line. It
// reads on open, after each change and on `voice_rooms_changed`; no polling.
// Never `fetchProjects()`.

import { useCallback, useEffect, useState } from 'react'
import { openUrl } from '@tauri-apps/plugin-opener'
import { Button, Callout, Checkbox } from '@/components/ui'
import { primaryScope } from '@/kessel/server-scope'
import { errorMessage } from '@/lib/daemon-http-error'
import { fetchVoiceRooms, fetchVoiceStatus, renameVoiceRoom, setVoiceRoomCount } from '@/lib/voice/voice-api'
import { hostingStateText, type VoiceRoomsList, type VoiceStatus } from '@/lib/voice/voice-wire'
import { onVoiceRoomsChanged } from '@/stores/session-events'
import { useVoice, voice } from '@/stores/voice'
import { useSettingsStore } from '@/stores/settings'
import type { SettingEntry } from '../searchManifest'

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

export function VoiceRoomsSection(): React.JSX.Element {
  const [rooms, setRooms] = useState<VoiceRoomsList | null>(null)
  const [status, setStatus] = useState<VoiceStatus | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [count, setCount] = useState<number | null>(null)
  const [closeNow, setCloseNow] = useState(false)
  const [busy, setBusy] = useState(false)
  const [names, setNames] = useState<Record<string, string>>({})
  const joinMuted = useVoice((s) => s.prefs.joinMuted)

  const load = useCallback(async () => {
    const scope = primaryScope()
    try {
      const list = await fetchVoiceRooms(scope)
      setRooms(list)
      setError(null)
      if (list.canManage) {
        // Owner/Admin (§9.4; the route's floor too); a Member never asks.
        const st = await fetchVoiceStatus(scope)
        setStatus(st)
        setCount((c) => c ?? st.count)
      }
    } catch (err) {
      setError(errorMessage(err))
    }
  }, [])

  useEffect(() => {
    void load()
    return onVoiceRoomsChanged(primaryScope(), () => void load())
  }, [load])

  // A server that can't host has no rooms to change, whoever is asking.
  const manage = rooms?.canManage === true && rooms.hosting.state !== 'not_linux'
  const hosting = status?.hosting ?? rooms?.hosting ?? null
  const line = hosting ? hostingStateText(hosting) : null
  const current = status?.count ?? null
  const lowering = current !== null && count !== null && count < current
  const occupiedDropped =
    lowering && rooms ? rooms.rooms.slice(count ?? 0).some((r) => r.occupants.length > 0) : false

  const applyCount = async (): Promise<void> => {
    if (count === null || count === current) return
    setBusy(true)
    try {
      await setVoiceRoomCount(primaryScope(), count, lowering && closeNow)
      setCloseNow(false)
      await load()
      void voice.refreshRooms()
    } catch (err) {
      setError(errorMessage(err))
    } finally {
      setBusy(false)
    }
  }

  const rename = async (roomId: string): Promise<void> => {
    const name = (names[roomId] ?? '').trim()
    if (!name) return
    setBusy(true)
    try {
      await renameVoiceRoom(primaryScope(), roomId, name)
      setNames((n) => {
        const next = { ...n }
        delete next[roomId]
        return next
      })
      await load()
      void voice.refreshRooms()
    } catch (err) {
      setError(errorMessage(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div data-testid="voice-rooms-section" className="flex max-w-[640px] flex-col gap-4">
      <header>
        <h2 className="type-title text-[var(--color-text-primary)]">Voice Rooms</h2>
        <p className="mt-1 type-row text-[var(--color-text-secondary)]">
          Voice rooms on this server, for its owner, Admins and Members. Join them from the headset in the top bar.
        </p>
      </header>

      {error ? (
        <Callout tone="error" data-testid="voice-rooms-error">
          {error}
        </Callout>
      ) : null}

      {line ? (
        <p data-testid="voice-rooms-hosting" data-state={hosting?.state} className="type-row text-[var(--color-text-secondary)]">
          {line}
          {hosting?.state === 'not_pro' ? (
            <>
              {' '}
              <button
                type="button"
                className="underline hover:text-[var(--color-text-primary)]"
                onClick={() => void openUrl('https://k2.dev/dashboard').catch((e: unknown) => setError(errorMessage(e)))}
              >
                Plans
              </button>
            </>
          ) : null}
          {hosting?.state === 'sew_off' || hosting?.state === 'voice_off' || hosting?.state === 'sew_start_failed' ? (
            <>
              {' '}
              <button
                type="button"
                className="underline hover:text-[var(--color-text-primary)]"
                onClick={() => useSettingsStore.getState().setSection('sew')}
              >
                Chat / Voice
              </button>
            </>
          ) : null}
          {hosting?.detail ? (
            <span data-testid="voice-rooms-hosting-detail" className="mt-0.5 block type-hint text-[var(--color-text-muted)]">
              {hosting.detail}
            </span>
          ) : null}
        </p>
      ) : null}

      {manage ? (
        <section className="flex flex-col gap-2">
          <h3 className="type-heading text-[var(--color-text-primary)]">Rooms</h3>
          <div className="flex items-center gap-2">
            <Button
              data-testid="voice-rooms-minus"
              disabled={busy || count === null || count <= 0}
              onClick={() => setCount((c) => (c === null ? c : Math.max(0, c - 1)))}
            >
              −
            </Button>
            <span data-testid="voice-rooms-count" className="w-6 text-center font-mono type-row">
              {count ?? '–'}
            </span>
            <Button
              data-testid="voice-rooms-plus"
              disabled={busy || count === null || count >= MAX_ROOMS}
              onClick={() => setCount((c) => (c === null ? c : Math.min(MAX_ROOMS, c + 1)))}
            >
              +
            </Button>
            <Button data-testid="voice-rooms-apply" disabled={busy || count === null || count === current} onClick={() => void applyCount()}>
              Save
            </Button>
          </div>
          {lowering ? (
            <div className="type-hint text-[var(--color-text-muted)]">
              <p>Removed rooms close when empty; people in them stay until they leave.</p>
              {occupiedDropped ? (
                <label className="mt-1 flex items-center gap-2 text-[var(--color-text-secondary)]">
                  <Checkbox data-testid="voice-rooms-close-now" checked={closeNow} onChange={(e) => setCloseNow(e.currentTarget.checked)} />
                  Close now: a 10 s notice, then everyone in them is disconnected
                </label>
              ) : null}
            </div>
          ) : null}
        </section>
      ) : null}

      {rooms ? (
        <ul className="flex flex-col gap-1" data-testid="voice-rooms-list">
          {rooms.rooms.map((r) => (
            <li key={r.id} data-testid="voice-rooms-row" className="flex items-center gap-2 type-row">
              {manage ? (
                <>
                  <input
                    aria-label={`Name of ${r.name}`}
                    className="w-[200px] border border-[var(--color-border)] bg-[var(--color-bg-elevated)] px-2 py-0.5 type-control"
                    value={names[r.id] ?? r.name}
                    maxLength={64}
                    onChange={(e) => {
                      const v = e.currentTarget.value
                      setNames((n) => ({ ...n, [r.id]: v }))
                    }}
                  />
                  {names[r.id] !== undefined && names[r.id].trim() !== r.name && names[r.id].trim() !== '' ? (
                    <Button disabled={busy} onClick={() => void rename(r.id)}>
                      Rename
                    </Button>
                  ) : null}
                </>
              ) : (
                <span className="text-[var(--color-text-primary)]">{r.name}</span>
              )}
              <span className="type-hint text-[var(--color-text-muted)]">
                {r.occupants.length} {r.occupants.length === 1 ? 'person' : 'people'} now
                {r.closing ? (r.closesAt !== null ? ' · Closing now' : ' · Closing when empty') : ''}
              </span>
            </li>
          ))}
        </ul>
      ) : null}

      {status?.mediaTier ? (
        <p data-testid="voice-rooms-media" className="type-hint text-[var(--color-text-muted)]">
          {status.mediaTier === 'tunnel'
            ? "Media mode: through K2's tunnel. This box's address isn't published."
            : "Media mode: direct. This box's public address is in DNS and in every join answer, so people in a call can see it."}
        </p>
      ) : null}

      <section className="flex flex-col gap-1 border-t border-[var(--color-border)] pt-3">
        <h3 className="type-heading text-[var(--color-text-primary)]">On this computer</h3>
        <label className="flex items-center gap-2 type-row text-[var(--color-text-secondary)]">
          <Checkbox data-testid="voice-join-muted" checked={joinMuted} onChange={(e) => voice.setPrefs({ joinMuted: e.currentTarget.checked })} />
          Join voice rooms with my microphone off
        </label>
      </section>
    </div>
  )
}
