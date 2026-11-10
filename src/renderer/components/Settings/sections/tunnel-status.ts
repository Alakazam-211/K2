// Settings → Tunnel: what the status card says. One answer to "can this
// server be reached from outside, at which address, and what do I do next",
// worked out from the daemon's tunnel status and config plus what this
// window is doing (signing in, turning on). Pure, so every state is tested.

import type { HealthTone } from './ConnectionsParts'

export type TunnelState =
  | 'loading'
  | 'read-failed'
  | 'not-set-up'
  | 'ready'
  | 'paused'
  | 'released'
  | 'connecting'
  | 'reachable'
  | 'repair'
  | 'failed'
  | 'missing-helper'

/** The one next step the card offers. */
export type TunnelAction = 'turn-on' | 'turn-off' | 'try-again' | 'repair' | 'check-again' | null

export interface TunnelFacts {
  /** The daemon answered GET /cli/tunnel/status at least once. */
  loaded: boolean
  /** Why the last status read failed (null = it didn't). */
  readError: string | null
  running: boolean
  /** false = the person turned it off (persists across restarts). */
  enabled?: boolean
  /** This computer gave up its address. */
  released?: boolean
  frpcInstalled: boolean
  /** A token is saved, so there is something to turn on. */
  tokenSet: boolean
  /** The address label (`alex` for alex.k2.dev), '' when none. */
  subdomain: string
  signedIn: boolean
  /** The signed-in account owns at least one address. */
  hasAddresses: boolean
  /** A Turn on or address switch is in flight from this window. */
  starting: boolean
  /** Why the last Turn on failed (null = it didn't). */
  startError: string | null
  /** Running, but the encrypted listener holds another name's certificate. */
  listenerName: string | null
}

export interface TunnelSummary {
  state: TunnelState
  tone: HealthTone
  /** The status word beside the dot. */
  word: string
  /** One plain sentence: what this means and what to do. */
  hint: string
  action: TunnelAction
  /** The server's own reason, shown quietly after the hint. */
  detail: string | null
}

export const hostOf = (sub: string): string => `${sub}.k2.dev`

export function summarizeTunnel(f: TunnelFacts): TunnelSummary {
  const sub = f.subdomain.trim()
  const host = sub ? hostOf(sub) : ''
  const base = { detail: null as string | null }
  if (!f.loaded) {
    if (f.readError) return { ...base, state: 'read-failed', tone: 'error', word: 'Unknown', hint: '', action: 'try-again', detail: f.readError }
    return { ...base, state: 'loading', tone: 'neutral', word: 'Checking…', hint: '', action: null }
  }
  if (f.starting) {
    return {
      ...base,
      state: 'connecting',
      tone: 'neutral',
      word: 'Connecting…',
      hint: host ? `Opening the tunnel to ${host}. This takes a few seconds.` : 'Opening the tunnel. This takes a few seconds.',
      action: null,
    }
  }
  if (f.running) {
    if (f.listenerName && sub && f.listenerName.toLowerCase() !== sub.toLowerCase()) {
      return {
        ...base,
        state: 'repair',
        tone: 'warn',
        word: 'Needs a repair',
        hint: `The tunnel is on, but its security certificate is still for ${hostOf(f.listenerName)}, so apps can't connect directly. Repair it; the tunnel stays on.`,
        action: 'repair',
      }
    }
    return {
      ...base,
      state: 'reachable',
      tone: 'ok',
      word: 'Reachable',
      hint: 'Your phone and other computers can reach this server at this address. Add it under Connected Servers on another computer, or pair K2 Companion.',
      action: 'turn-off',
    }
  }
  if (!f.frpcInstalled) {
    return {
      ...base,
      state: 'missing-helper',
      tone: 'error',
      word: "Can't turn on",
      hint: "The tunnel needs a helper program called frpc, and it isn't on this computer. Install it with Homebrew or from the frp releases page, then check again.",
      action: 'check-again',
    }
  }
  if (f.startError) {
    return {
      ...base,
      state: 'failed',
      tone: 'error',
      word: "Couldn't turn on",
      hint: host
        ? `K2 couldn't open the tunnel to ${host}. Check this computer is online, then try again.`
        : "K2 couldn't open the tunnel. Check this computer is online, then try again.",
      action: 'try-again',
      detail: f.startError,
    }
  }
  if (f.released) {
    return {
      ...base,
      state: 'released',
      tone: 'warn',
      word: 'No address',
      hint: f.signedIn
        ? 'This computer gave up its address. Choose one below to use the tunnel here again.'
        : 'This computer gave up its address. Sign in below and choose one to use the tunnel here again.',
      action: null,
    }
  }
  if (!f.tokenSet && !sub) {
    return {
      ...base,
      state: 'not-set-up',
      tone: 'neutral',
      word: 'Not set up',
      hint: !f.signedIn
        ? "Not reachable from the internet yet. Sign in to your k2.dev account below to choose this server's address."
        : f.hasAddresses
          ? "Not reachable from the internet yet. Choose this server's address below, then turn it on."
          : 'Not reachable from the internet yet. Your k2.dev account has no addresses; get one at k2.dev.',
      action: null,
    }
  }
  if (f.enabled === false) {
    return {
      ...base,
      state: 'paused',
      tone: 'neutral',
      word: 'Off',
      hint: 'You turned the tunnel off. It stays off, even after a restart, until you turn it on here.',
      action: 'turn-on',
    }
  }
  return {
    ...base,
    state: 'ready',
    tone: 'neutral',
    word: 'Off',
    hint: host
      ? `Not reachable from the internet. Turn it on to reach this server at ${host}.`
      : 'Not reachable from the internet. Turn it on to reach this server.',
    action: 'turn-on',
  }
}
