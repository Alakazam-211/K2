// DM calls on Home: the logic behind the call pieces (prd-home-people-
// channels-v1 §11a.2–§11a.4). `HomeChatCall.tsx` draws; this decides.
//
// - `useDmCall`: one DM's call for the open chat view: the Call button, the
//   caller's "Calling…" bar, the in-call strip, Call back.
// - `HomeChatCallLayer`: the ring cards of every server this window follows,
//   with the ring sound (never for a silent ring). Mounted once per window;
//   a ring stops in every window when the daemon says it stopped.
//
// Media is the voice controller's: a DM call joins as the room
// `dm:<conversation id>`. The daemon rings when nobody rings for this person
// in the DM, and answers the ring when one does.

import { useCallback, useEffect, useMemo, useRef, useState, type JSX } from 'react'
import { voice, useVoice } from '@/stores/voice'
import { useToastStore } from '@/stores/toast'
import { daemonErrorText } from '@/lib/daemon-error'
import {
  cancelDmCall,
  convOfDmRoom,
  declineDmCall,
  dmCallRoom,
  ringDmCallAgain,
} from '@/lib/home-chat/calls'
import { startRingSound, stopRingSound } from '@/lib/home-chat/ring-sound'
import { CALL_ENDED_SHOW_MS, ringsNow, useHomeChatCallsStore } from '@/stores/home-chat-calls'
import { refreshHomeChatPeople, scopeForChatHost, useHomeChatStore } from '@/stores/home-chat'
import { useConnectHostStore } from '@/stores/connect-host'
import { activeHomeHostKey } from '@/lib/home-address'
import { HomeChatCallingBar, HomeChatCallStrip, HomeChatRingCard } from './HomeChatCall'
import { serverLabelFor } from './serverLabel'
import { useBrowserOverlay } from '@/lib/browser-overlay'

function toastError(e: unknown): void {
  useToastStore.getState().addToast(daemonErrorText(e) || String(e), 'error')
}

/** Join the DM call as `name`: a room call in progress is ended first, after
 *  the person says yes (Sew keeps one voice entry per person). */
async function joinDm(hostKey: string, conv: string, name: string): Promise<void> {
  const cur = voice.store.getState().call
  if (cur && convOfDmRoom(cur.roomId) !== conv) {
    if (!window.confirm(`Calling ${name} ends your call in ${cur.roomName}. Call now?`)) return
  }
  await voice.join(hostKey, dmCallRoom(conv, name))
}

export interface DmCallView {
  /** The caller's bar, or null. */
  calling: 'joining' | 'ringing' | 'busy' | 'declined' | 'missed' | null
  /** The in-call strip: here (with seconds), elsewhere, or none. */
  strip: { seconds: number | null; muted: boolean; elsewhere: string | null } | null
  call: () => void
  cancel: () => void
  callAgain: () => void
  leave: () => void
  toggleMute: () => void
}

/** One DM's call, for the open chat view. `me` is the person's Sew member id. */
export function useDmCall(hostKey: string, conv: string | null, name: string, me: string | null): DmCallView {
  const voiceCall = useVoice((s) => s.call)
  const live = useHomeChatCallsStore((s) => (conv ? s.calls[`${hostKey}|${conv}`] : undefined))
  const ended = useHomeChatCallsStore((s) => (conv ? s.ended[`${hostKey}|${conv}`] : undefined))
  const [now, setNow] = useState(() => Date.now())
  const mine = !!voiceCall && !!conv && voiceCall.hostKey === hostKey && convOfDmRoom(voiceCall.roomId) === conv

  // The strip's clock, and the moment an ended line goes.
  useEffect(() => {
    if (!mine && !ended) return
    const t = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(t)
  }, [mine, ended])

  // The call ended on the server (declined, no answer, the other side left):
  // this window leaves too, so the seat and the mic are let go.
  useEffect(() => {
    if (mine && ended && voiceCall?.phase !== 'joining' && !live) void voice.leave('call_ended')
  }, [mine, ended, live, voiceCall?.phase])

  const other = live?.rings.find((r) => r.memberId !== me)
  const iStarted = !!live && live.startedBy === me
  const someoneJoined = !!live && live.participantIds.some((p) => p !== me)
  let calling: DmCallView['calling'] = null
  if (mine && voiceCall?.phase === 'joining' && !live) calling = 'joining'
  else if (mine && iStarted && !someoneJoined && (other?.state === 'ringing' || other?.state === 'busy')) calling = other.state === 'busy' ? 'busy' : 'ringing'
  else if (!mine && ended && ended.at + CALL_ENDED_SHOW_MS > now && (ended.reason === 'declined' || ended.reason === 'no_answer')) {
    calling = ended.reason === 'declined' ? 'declined' : 'missed'
  }

  let strip: DmCallView['strip'] = null
  if (mine && calling === null && voiceCall) {
    const seconds = voiceCall.connectedAt !== null ? Math.max(0, Math.floor((now - voiceCall.connectedAt) / 1000)) : null
    strip = { seconds, muted: voiceCall.muted, elsewhere: null }
  } else if (!mine && live && me && live.participantIds.includes(me)) {
    strip = { seconds: null, muted: false, elsewhere: 'another device' }
  }

  const call = useCallback(() => {
    if (conv) joinDm(hostKey, conv, name).catch(toastError)
  }, [hostKey, conv, name])
  const cancel = useCallback(() => {
    if (!conv) return
    const id = live?.callId
    const done = (): void => void voice.leave('cancelled')
    if (!id) return done()
    cancelDmCall(scopeForChatHost(hostKey), conv, id).then(done, (e: unknown) => {
      toastError(e)
      done()
    })
  }, [hostKey, conv, live?.callId])
  const callAgain = useCallback(() => {
    if (!conv) return
    if (live && !live.ended) ringDmCallAgain(scopeForChatHost(hostKey), conv, live.callId).catch(toastError)
    else joinDm(hostKey, conv, name).catch(toastError)
  }, [hostKey, conv, name, live])
  const leave = useCallback(() => void voice.leave(), [])
  const toggleMute = useCallback(() => {
    const c = voice.store.getState().call
    if (c) voice.setMuted(!c.muted)
  }, [])

  return { calling, strip, call, cancel, callAgain, leave, toggleMute }
}

/** The DM's call bars, drawn under its header. */
export function HomeChatDmCallBars({ view, name }: { view: DmCallView; name: string }): JSX.Element | null {
  if (view.calling) {
    return <HomeChatCallingBar name={name} state={view.calling} onCancel={view.cancel} onCallAgain={view.callAgain} />
  }
  if (view.strip) {
    return <HomeChatCallStrip seconds={view.strip.seconds} muted={view.strip.muted} elsewhere={view.strip.elsewhere} onMute={view.toggleMute} onLeave={view.leave} />
  }
  return null
}

/** Every ring card this window shows, top right, with the ring sound. */
export function HomeChatCallLayer(): JSX.Element | null {
  const connectedKey = useConnectHostStore((s) => activeHomeHostKey(s.activeHost))
  const savedHosts = useConnectHostStore((s) => s.hosts)
  const serverLabelOf = (hostKey: string): string | null => (hostKey === connectedKey ? null : serverLabelFor(hostKey, savedHosts))
  const ringMap = useHomeChatCallsStore((s) => s.rings)
  const rings = useMemo(() => ringsNow(ringMap), [ringMap])
  const byHost = useHomeChatStore((s) => s.byHost)
  const [busy, setBusy] = useState<string | null>(null)
  const loud = rings.some((r) => !r.silent)

  useEffect(() => {
    if (loud) startRingSound()
    else stopRingSound()
    return () => stopRingSound()
  }, [loud])

  // The caller's name comes from that server's people list.
  const hosts = useMemo(() => [...new Set(rings.map((r) => r.hostKey))].sort().join('\n'), [rings])
  useEffect(() => {
    for (const h of hosts ? hosts.split('\n') : []) void refreshHomeChatPeople(h)
  }, [hosts])

  // The ring cards float over whatever room is open, a live Browser page
  // included, so they claim their box like any popover.
  const layerRef = useRef<HTMLDivElement>(null)
  useBrowserOverlay({ active: rings.length > 0, ref: layerRef, kind: 'popover', watch: rings.length })

  if (rings.length === 0) return null
  return (
    <div ref={layerRef} className="pointer-events-none fixed right-4 top-14 z-50 flex flex-col gap-2" data-testid="home-chat-call-layer">
      {rings.map((r) => {
        const people = byHost[r.hostKey]?.people?.people ?? []
        const who = people.find((p) => p.memberId !== null && p.memberId === r.startedBy) ?? null
        const name = who?.displayName ?? 'Someone'
        const k = `${r.hostKey}|${r.callId}`
        return (
          <div key={k} className="pointer-events-auto">
            <HomeChatRingCard
              callerName={name}
              picture={who?.picture ?? null}
              silent={r.silent}
              serverLabel={serverLabelOf(r.hostKey)}
              busy={busy === k}
              onAccept={() => {
                setBusy(k)
                joinDm(r.hostKey, r.conv, name)
                  .catch(toastError)
                  .finally(() => setBusy(null))
              }}
              onDecline={() => {
                setBusy(k)
                declineDmCall(scopeForChatHost(r.hostKey), r.conv, r.callId)
                  .catch(toastError)
                  .finally(() => setBusy(null))
              }}
            />
          </div>
        )
      })}
    </div>
  )
}
