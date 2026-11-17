// DM calls on Home: the logic behind the call pieces (prd-home-people-
// channels-v1 §11a.2–§11a.4). `HomeChatCall.tsx` draws; this decides.
//
// - `useDmCall`: one DM's call for the open chat view: the Call button, the
//   caller's "Calling…" bar, the in-call strip, Call back.
// - `HomeChatCallLayer`: the ring cards of every server this window follows,
//   with the ring sound (never for a silent ring). Mounted once per window;
//   a ring stops in every window when the daemon says it stopped.
// - `HomeChatRingWatch`: what makes a window follow a server's rings: it
//   holds the Home chat events socket of every server a ring can come from
//   (`ringWatchHosts`), whatever page is open. Without it the socket lived
//   only while a chat row or the chat view was on screen, so a callee on
//   another page never heard the ring.
// - `HomeChatCallRings`: the two together. App.tsx's `CallRingHost` draws
//   it, and ConnectionGate mounts that beside the keyed App in every branch,
//   so the cards show over Settings, Zen and focus mode, on the Voice & Chat
//   only screen, and survive a server switch.
//
// Media is the voice controller's: a DM call joins as the room
// `dm:<conversation id>`. The daemon rings when nobody rings for this person
// in the DM, and answers the ring when one does.

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type JSX } from 'react'
import { voice, useVoice } from '@/stores/voice'
import { useToastStore } from '@/stores/toast'
import { daemonErrorText } from '@/lib/daemon-error'
import {
  cancelDmCall,
  convOfDmRoom,
  declineDmCall,
  dmCallRoom,
  dmRoomId,
  ringDmCallAgain,
} from '@/lib/home-chat/calls'
import { startRingSound, stopRingSound } from '@/lib/home-chat/ring-sound'
import { callerEndedLine, ringsNow, useHomeChatCallsStore } from '@/stores/home-chat-calls'
import { refreshHomeChatPeople, retainHomeChatHost, scopeForChatHost, useHomeChatStore } from '@/stores/home-chat'
import { useConnectHostStore } from '@/stores/connect-host'
import { useHomesStore } from '@/stores/homes'
import { activeHomeHostKey } from '@/lib/home-address'
import { homeChatTarget } from '@/lib/home-chat/address'
import { HOME_CHAT_FEATURE } from '@/lib/home-chat/api'
import { homeHostKey } from '@/lib/host-key'
import { hostPool } from '@/lib/host-pool-instance'
import { hostIsVoiceChatOnly } from '@/lib/voice-chat-only'
import { CALL_RING_Z } from '@/lib/overlay-layers'
import { useStore } from 'zustand'
import { HomeChatCallingBar, HomeChatCallStrip, HomeChatRingCard, ringLayerTop } from './HomeChatCall'
import { serverLabelFor } from './serverLabel'
import { useBrowserOverlay } from '@/lib/browser-overlay'
import { ConversationCallPlace } from '@/components/Voice/CallPlace'
import { clientRectToCss } from '@/lib/app-zoom'

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
  else if (!mine) calling = callerEndedLine(ended, me, now)

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

/** The DM's call bars, drawn under its header. In the call here, the call
 *  lives at the top of its conversation (ConversationCallPlace: the stage
 *  with its video, or the slim bar); where this app has no engine for it
 *  (the web client's seats), the DM's own strip. */
export function HomeChatDmCallBars({ view, name, hostKey, conv }: { view: DmCallView; name: string; hostKey: string; conv: string | null }): JSX.Element | null {
  if (view.calling) {
    return <HomeChatCallingBar name={name} state={view.calling} onCancel={view.cancel} onCallAgain={view.callAgain} />
  }
  if (view.strip) {
    const strip = <HomeChatCallStrip seconds={view.strip.seconds} muted={view.strip.muted} elsewhere={view.strip.elsewhere} onMute={view.toggleMute} onLeave={view.leave} />
    if (view.strip.elsewhere || !conv) return strip
    return <ConversationCallPlace hostKey={hostKey} roomId={dmRoomId(conv)} fallback={strip} />
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

  // Under the open chat's header, so its Call and Members stay in reach;
  // just under the top bar on a page with no chat header. A ring shows on
  // every page, so the header can come and go while it rings: look again
  // now and then as well as on resize.
  const [top, setTop] = useState(() => ringLayerTop(null))
  useLayoutEffect(() => {
    if (rings.length === 0) return
    const read = (): void => {
      const header = document.querySelector('[data-testid="home-chat-header"]')
      setTop(ringLayerTop(header ? clientRectToCss(header.getBoundingClientRect()).bottom : null))
    }
    read()
    window.addEventListener('resize', read)
    const every = window.setInterval(read, 500)
    return () => {
      window.removeEventListener('resize', read)
      window.clearInterval(every)
    }
  }, [rings.length])

  if (rings.length === 0) return null
  return (
    <div
      ref={layerRef}
      className="pointer-events-none fixed right-4 flex flex-col gap-2"
      style={{ top, zIndex: CALL_RING_Z }}
      data-testid="home-chat-call-layer"
    >
      {rings.map((r) => {
        const people = byHost[r.hostKey]?.people?.people ?? []
        const who = people.find((p) => p.memberId !== null && p.memberId === r.startedBy) ?? null
        const name = who?.displayName ?? 'Someone'
        const k = `${r.hostKey}|${r.callId}`
        return (
          <div key={k} className="pointer-events-auto">
            <HomeChatRingCard
              callerName={name}
              username={who?.username ?? name}
              picture={who?.picture ?? null}
              silent={r.silent}
              serverLabel={serverLabelOf(r.hostKey)}
              busy={busy === k}
              onAccept={() => {
                setBusy(k)
                joinDm(r.hostKey, r.conv, name)
                  .catch(toastError)
                  .finally(() => setBusy(null))
                // On the Voice & Chat only screen (this window's server is
                // one where the login is Voice & Chat only), the call's
                // controls are on This computer, as for its voice rooms.
                const active = useConnectHostStore.getState().activeHost
                if (active !== 'local' && hostIsVoiceChatOnly(active)) useConnectHostStore.getState().pickHost('local')
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

/** The servers a DM call can ring this window from, sorted: the window's own
 *  server; every server whose `/boot-status` says Home chat; every server a
 *  person or channel row on ANY Home names (not only the Home on screen);
 *  every saved server where this login is Voice & Chat only (it is there to
 *  talk and call). A server whose chat is off answers its status once, and
 *  again now and then (`retainHomeChatHost`); no socket opens there. */
export function ringWatchHosts(
  connectedKey: string,
  poolEntries: Record<string, { boot?: { features?: readonly string[] } | null } | undefined>,
  homes: ReadonlyArray<{ rows: ReadonlyArray<{ kind?: string; address: string }> }>,
  savedHosts: ReadonlyArray<{ hostname: string; port: number; secure: boolean; access?: string }>,
): string[] {
  const out = new Set<string>()
  if (connectedKey) out.add(connectedKey)
  for (const [key, e] of Object.entries(poolEntries)) {
    if (e?.boot?.features?.includes(HOME_CHAT_FEATURE)) out.add(key)
  }
  for (const home of homes) {
    for (const row of home.rows) {
      const t = homeChatTarget(row)
      if (t) out.add(t.hostKey)
    }
  }
  for (const h of savedHosts) {
    if (hostIsVoiceChatOnly(h)) out.add(homeHostKey(h))
  }
  return [...out].sort()
}

/** Hold the Home chat events socket of every server in `ringWatchHosts`
 *  while mounted, so a ring reaches this window on any page. Counted with
 *  the rows and the chat view: still one socket per server per window. */
export function HomeChatRingWatch(): null {
  const connectedKey = useConnectHostStore((s) => activeHomeHostKey(s.activeHost))
  const savedHosts = useConnectHostStore((s) => s.hosts)
  const homes = useHomesStore((s) => s.homes)
  // Only the keys that say Home chat, as one string, so a health poll that
  // changes nothing else never re-runs the effect.
  const poolKeys = useStore(hostPool.store, (s) =>
    Object.entries(s.entries)
      .filter(([, e]) => e?.boot?.features?.includes(HOME_CHAT_FEATURE))
      .map(([k]) => k)
      .sort()
      .join('\n'),
  )
  const hosts = useMemo(() => {
    const pool = Object.fromEntries((poolKeys ? poolKeys.split('\n') : []).map((k) => [k, { boot: { features: [HOME_CHAT_FEATURE] } }]))
    return ringWatchHosts(connectedKey, pool, homes, savedHosts).join('\n')
  }, [connectedKey, poolKeys, homes, savedHosts])
  useEffect(() => {
    const releases = (hosts ? hosts.split('\n') : []).map((h) => retainHomeChatHost(h))
    return () => {
      for (const release of releases) release()
    }
  }, [hosts])
  return null
}

/** The window's DM call rings: the socket holder and the ring cards
 *  (App.tsx's `CallRingHost` mounts it). */
export function HomeChatCallRings(): JSX.Element {
  return (
    <>
      <HomeChatRingWatch />
      <HomeChatCallLayer />
    </>
  )
}
