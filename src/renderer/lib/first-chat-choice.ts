// The first pinned chat asks which LLM to start (0.46.11).
//
// On a server that has never had a pinned chat, the daemon answers a new
// agent's `ensure-pinned-chat` with 409 `harness_choice_needed` and the
// harnesses to pick from, and starts nothing (`pinned_chat_choice.rs`). The
// Chat tab draws the list; the pick goes back as `harness` on the same
// ensure, and the daemon saves it as the agent's default and starts it.

import { httpDetailsOf } from '@/lib/daemon-http-error'

export const HARNESS_CHOICE_NEEDED = 'harness_choice_needed'
export const HARNESS_NOT_INSTALLED = 'harness_not_installed'

/** One harness the person can start the chat on (the daemon's `Choice`). */
export interface FirstChatHarness {
  harness: string
  label: string
  /** On the PATH agents run with on that server. */
  installed: boolean
  /** K2 can install it there. */
  install: boolean
  /** A K2 sign-in for it is signed in, or an API token is saved. */
  signedIn: boolean
  /** K2 can sign it in there. */
  signIn: boolean
}

function codeOf(err: unknown): string | null {
  const code = (err as { code?: unknown } | null)?.code
  return typeof code === 'string' ? code : null
}

function asHarness(raw: unknown): FirstChatHarness | null {
  if (!raw || typeof raw !== 'object') return null
  const r = raw as Record<string, unknown>
  if (typeof r.harness !== 'string' || r.harness === '') return null
  return {
    harness: r.harness,
    label: typeof r.label === 'string' && r.label !== '' ? r.label : r.harness,
    installed: r.installed === true,
    install: r.install === true,
    signedIn: r.signedIn === true,
    signIn: r.signIn === true,
  }
}

/** The harnesses to pick from when `err` is the daemon's "choose first"
 *  refusal, else null. */
export function firstChatChoices(err: unknown): FirstChatHarness[] | null {
  if (codeOf(err) !== HARNESS_CHOICE_NEEDED) return null
  const list = httpDetailsOf(err)?.harnesses
  if (!Array.isArray(list)) return []
  return list.map(asHarness).filter((h): h is FirstChatHarness => h !== null)
}

/** The daemon refused a pick because its CLI isn't installed. */
export function isHarnessNotInstalled(err: unknown): boolean {
  return codeOf(err) === HARNESS_NOT_INSTALLED
}

/** What a row says about its harness. */
export function harnessStatusText(h: FirstChatHarness): string {
  if (!h.installed) return h.install ? 'Not installed. Install it to start here.' : 'Not installed on this server.'
  if (h.signedIn) return 'Signed in.'
  if (h.signIn) return 'Not signed in yet. Sign in first, or start and sign in inside the chat.'
  return 'Installed.'
}
