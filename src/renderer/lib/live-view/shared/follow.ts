// Follow, as one state machine (the mocks' one control):
//
//   idle       "Follow Atlas" (filled), aimed at the agent at work
//   following  "Following Brook · Stop": one control, tinted with that
//              person's colour, that always names them
//   away       you looked away (scrolled, picked a view, played): "Back to
//              Brook" (filled). The timeline puts Back in its transport.
//   left       the person you followed left: "Sam left the file" for 4 s,
//              and you keep the view
//
// Gone once the agent is done (unless you follow a person). Pure: the view
// keeps the state and the clock.

import { personById, type LivePerson } from './people'

export type FollowState =
  | { kind: 'idle' }
  | { kind: 'following'; who: string }
  | { kind: 'away'; who: string }
  | { kind: 'left'; who: string; name: string; until: number }

export type FollowAction =
  | { type: 'follow'; who: string }
  | { type: 'stop' }
  | { type: 'look-away' }
  | { type: 'back' }
  | { type: 'left'; who: string; name: string; now: number }
  | { type: 'tick'; now: number }

export const FOLLOW_IDLE: FollowState = { kind: 'idle' }

/** How long "<name> left the file" shows. */
export const LEFT_NOTE_MS = 4000

export function followReducer(s: FollowState, a: FollowAction, people?: readonly LivePerson[]): FollowState {
  switch (a.type) {
    case 'follow':
      if (people && personById(people, a.who)?.self) return s
      return s.kind === 'following' && s.who === a.who ? s : { kind: 'following', who: a.who }
    case 'stop':
      return s.kind === 'idle' ? s : FOLLOW_IDLE
    case 'look-away':
      return s.kind === 'following' ? { kind: 'away', who: s.who } : s
    case 'back':
      return s.kind === 'away' ? { kind: 'following', who: s.who } : s
    case 'left':
      return (s.kind === 'following' || s.kind === 'away') && s.who === a.who ? { kind: 'left', who: a.who, name: a.name, until: a.now + LEFT_NOTE_MS } : s
    case 'tick':
      return s.kind === 'left' && a.now >= s.until ? FOLLOW_IDLE : s
  }
}

export interface FollowView {
  control: 'follow' | 'following' | 'back' | 'none'
  /** "Follow Atlas", "Following Brook", "Back to Brook". */
  label: string
  /** The cut control's second word while following. */
  stop?: string
  /** Whom the control acts on. */
  target?: string
  /** The followed person's colour (the control's tint). */
  colour?: string
  /** "Sam left the file", while it shows. */
  note?: string
}

export interface FollowContext {
  people: readonly LivePerson[]
  /** The agent at work: the default Follow target. */
  agent: string | null
  /** The agent has finished: Follow goes. */
  agentDone: boolean
}

export function followView(s: FollowState, ctx: FollowContext): FollowView {
  const who = s.kind === 'following' || s.kind === 'away' ? personById(ctx.people, s.who) : undefined
  if (who && s.kind === 'following') return { control: 'following', label: `Following ${who.name}`, stop: 'Stop', target: who.id, colour: who.colour }
  if (who && s.kind === 'away') return { control: 'back', label: `Back to ${who.name}`, target: who.id }
  const note = s.kind === 'left' ? `${s.name} left the file` : undefined
  const agent = personById(ctx.people, ctx.agent)
  if (!agent || ctx.agentDone) return { control: 'none', label: '', note }
  return { control: 'follow', label: `Follow ${agent.name}`, target: agent.id, note }
}
