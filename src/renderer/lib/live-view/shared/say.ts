// "Say something to X": a small composer to whom you follow (or the agent),
// with a chip naming what it points at (sent as its address). Its states:
// typing (Send quiet until there's text), sending, sent (the line shows in
// the row), a reply, a failure that keeps the words and offers Send again,
// a second failure, and off while not connected ("Back online to send").
// Pure: the view keeps the state; the source sends.

export type SayStatus = 'idle' | 'sending' | 'sent' | 'failed'

export interface SayState {
  draft: string
  status: SayStatus
  /** Failed tries of the message in the field. */
  tries: number
  /** The last line of the conversation, for the row. */
  last?: { from: 'you' | 'them'; text: string }
  /** The last send went through on Send again. */
  resent?: boolean
}

export type SayAction =
  | { type: 'draft'; text: string }
  | { type: 'send' }
  | { type: 'send-again' }
  | { type: 'sent' }
  | { type: 'failed' }
  | { type: 'reply'; text: string }

export const SAY_IDLE: SayState = { draft: '', status: 'idle', tries: 0 }

export function sayReducer(s: SayState, a: SayAction): SayState {
  switch (a.type) {
    case 'draft':
      return { ...s, draft: a.text, status: s.status === 'failed' ? 'failed' : s.status === 'sending' ? 'sending' : 'idle' }
    case 'send':
      if (!s.draft.trim() || s.status === 'sending' || s.status === 'failed') return s
      return { ...s, status: 'sending', resent: false }
    case 'send-again':
      return s.status === 'failed' && s.draft.trim() ? { ...s, status: 'sending', resent: true } : s
    case 'sent':
      if (s.status !== 'sending') return s
      return { draft: '', status: 'sent', tries: 0, last: { from: 'you', text: s.draft.trim() }, resent: s.resent }
    case 'failed':
      return s.status === 'sending' ? { ...s, status: 'failed', tries: s.tries + 1 } : s
    case 'reply':
      return { ...s, last: { from: 'them', text: a.text }, resent: false }
  }
}

export interface SayLine {
  tone: 'plain' | 'bad'
  /** "You → Atlas", "Atlas → You" (absent on a failure). */
  who?: string
  text: string
  /** "Send again" on a failure. */
  action?: string
  /** "Sent." after Send again went through. */
  note?: string
}

export interface SayView {
  placeholder: string
  /** The field is off (not connected). */
  disabled: boolean
  /** Send is the accent (there's text to send). */
  sendPrimary: boolean
  canSend: boolean
  line: SayLine | null
}

export interface SayContext {
  toName: string
  /** Not connected: the field is off. */
  offline: boolean
  /** Edits are paused (a declined save): you can still message. */
  paused: boolean
}

export function sayView(s: SayState, ctx: SayContext): SayView {
  const placeholder = ctx.offline ? 'Back online to send' : ctx.paused ? `Edits paused; you can still message ${ctx.toName}…` : `Say something to ${ctx.toName}…`
  const has = s.draft.trim().length > 0
  let line: SayLine | null = null
  if (s.status === 'failed')
    line = {
      tone: 'bad',
      text: s.tries > 1 ? 'Still couldn’t send (second try). Your message is kept here.' : 'Couldn’t send: the connection dropped. Your message is kept here.',
      action: 'Send again',
    }
  else if (s.last) {
    line = { tone: 'plain', who: s.last.from === 'you' ? `You → ${ctx.toName}` : `${ctx.toName} → You`, text: s.last.text }
    if (s.resent && s.last.from === 'you') line.note = 'Sent.'
  }
  const canSend = has && !ctx.offline && s.status !== 'sending' && s.status !== 'failed'
  return { placeholder, disabled: ctx.offline, sendPrimary: has && s.status !== 'failed', canSend, line }
}
