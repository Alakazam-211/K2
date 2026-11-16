// The notice bar's words. One notice at a time, as a full-width bar above
// the hero, each with what happened and one next step:
//
//   - the kit's four outside-save outcomes (`outside_save`): taken in, kept
//     both, declined (edits pause), missing;
//   - a refused verb (`verb_refused`): who couldn't do what, and who holds it;
//   - the connection: reconnecting (Retry now), trying (a disabled
//     "Trying…"), can't reconnect (Retry, Open the saved file), offline, and
//     back ("Back." with what came in);
//   - the file's size: large ("A large scene: …", Find), too large to watch
//     live (the saved version, Open the saved file).
//
// A connection notice outranks the rest; it is said once (here), and the
// header's chip only names the state. Copy from the council-passed mocks
// (live-timeline-v2 and live-scene-v2 notes, v2.5).

import type { OutsideSaveFrame } from '@k2/mpc-client'
import { plural } from './people'

/** An outside save as the shell needs it: the kit's frame without its envelope.
 *  `kept_both.copy.doc` is filled once the host knows the copy's document. */
export type OutsideSave = OutsideSaveFrame extends infer F ? (F extends { type: 'outside_save' } ? Omit<F, 'type' | 'at_ms' | 'doc'> : never) : never

export type ConnectionState = 'reconnecting' | 'trying' | 'cant' | 'offline' | 'back'

export type LiveNotice =
  | { kind: 'outside_save'; save: OutsideSave }
  | {
      kind: 'refused'
      actor: string
      verb: string
      target: string
      holder: string | null
      /** Why, in words, when no one holds it ("animation drives it"). */
      why?: string | null
      /** Whom "Ask …" asks (the holder, else the one refused), when the view knows; null: no one to ask. */
      ask?: { id: string; name: string } | null
      /** What was refused, as the say row's address when you ask. */
      address?: { label: string; address: string } | null
    }
  | { kind: 'connection'; state: ConnectionState; changes?: number; following?: string | null; retried?: boolean }

  | {
      kind: 'size'
      size: 'large' | 'too_large'
      /** What the file is, in a word ("scene"). */
      noun: string
      count: number
      /** What it counts ("objects"). */
      unit: string
      /** About where live editing stops. */
      limit?: number
      /** How they're grouped ("100 collections"): the list opens them closed. */
      groups?: { count: number; unit: string }
      /** The find field's name ("Find an object"). */
      find?: string
    }

export type NoticeActionId = 'retry' | 'open-saved-file' | 'open-copy' | 'history' | 'save-again' | 'ask' | 'find' | 'dismiss'

export interface NoticeAction {
  id: NoticeActionId
  label: string
  primary?: boolean
  disabled?: boolean
}

export interface NoticeView {
  tone: 'info' | 'warn' | 'bad'
  /** `alert` for what needs you, `status` for what doesn't. */
  role: 'status' | 'alert'
  title: string
  line: string
  /** A spinner instead of the tone's icon. */
  busy?: boolean
  actions: NoticeAction[]
  /** Title and line together (for checks and the accessible text). */
  text: string
}

export interface NoticeContext {
  fileName: string
  /** What stays on screen while reconnecting ("who’s watching, the changes and your playhead"). */
  kept?: string
}

const DISMISS: NoticeAction = { id: 'dismiss', label: 'Dismiss' }

function view(tone: NoticeView['tone'], role: NoticeView['role'], title: string, line: string, actions: NoticeAction[], busy?: boolean): NoticeView {
  return { tone, role, title, line, actions, ...(busy ? { busy } : {}), text: `${title} ${line}`.trim() }
}

function saveView(save: OutsideSave, ctx: NoticeContext): NoticeView {
  switch (save.outcome) {
    case 'ingested':
      return view('info', 'status', 'Saved outside K2 · now in the live file', 'The changes show as an outside change.', [DISMISS])
    case 'kept_both':
      return save.copy
        ? view('warn', 'alert', 'Saved outside K2: that save is now live.', 'What was live before is kept as a copy.', [
            { id: 'open-copy', label: 'Open the copy', primary: true },
            { id: 'history', label: 'History' },
            DISMISS,
          ])
        : view('warn', 'alert', 'Saved outside K2: that save is now live.', 'What was live before is kept in History.', [{ id: 'history', label: 'History' }, DISMISS])
    case 'declined':
      return view('bad', 'alert', 'Saved outside K2, and the file can’t be live any more.', `${save.message.trim() || 'K2 can’t watch this save live.'} Edits are paused.`, [
        { id: 'open-saved-file', label: 'Open the saved file', primary: true },
      ])
    case 'missing':
      return view('warn', 'alert', `${ctx.fileName} was moved or deleted outside K2.`, 'The live version is kept. Save it again to keep working on it.', [
        { id: 'save-again', label: 'Save it again', primary: true },
      ])
  }
}

function connectionView(n: Extract<LiveNotice, { kind: 'connection' }>, ctx: NoticeContext): NoticeView {
  const kept = ctx.kept ?? 'who’s watching and the changes'
  switch (n.state) {
    case 'reconnecting':
      return view('info', 'status', 'Reconnecting…', `What you see stays as it was: ${kept}. Changes made meanwhile come in when you’re back.`, [{ id: 'retry', label: 'Retry now' }], true)
    case 'trying':
      return view('info', 'status', 'Reconnecting…', 'Trying again now…', [{ id: 'retry', label: 'Trying…', disabled: true }], true)
    case 'cant':
      return view('bad', 'alert', 'Can’t reach this file’s server.', `${n.retried ? 'Tried again just now: still no answer. ' : ''}K2 stopped trying. You see things as they were; anything new shows when you’re back.`, [
        { id: 'retry', label: 'Retry', primary: true },
        { id: 'open-saved-file', label: 'Open the saved file' },
      ])
    case 'offline':
      return view('warn', 'status', 'You’re offline.', 'You see things as they were; anything new shows when you’re back online.', [{ id: 'retry', label: 'Retry' }])
    case 'back': {
      const n2 = n.changes ?? 0
      const came = n2 === 0 ? 'Nothing changed while you were away.' : `${plural(n2, 'change')} came in while you were away. ${n2 === 1 ? 'It’s' : 'They’re'} first in Changes.`
      return view('info', 'status', 'Back.', `${n.following ? `You’re following ${n.following} again. ` : ''}${came}`, [DISMISS])
    }
  }
}

const num = (n: number): string => n.toLocaleString('en-US')

function sizeView(n: Extract<LiveNotice, { kind: 'size' }>): NoticeView {
  if (n.size === 'too_large')
    return view('info', 'status', 'Too large to watch live', `This ${n.noun} has ${num(n.count)} ${n.unit}${n.limit ? `; live editing stops at about ${num(n.limit)}` : ''}. You see the last saved version, read-only.`, [
      { id: 'open-saved-file', label: 'Open the saved file' },
    ])
  const g = n.groups
  return view('info', 'status', `A large ${n.noun}: ${num(n.count)} ${n.unit}${g ? ` in ${num(g.count)} ${g.unit}` : ''}.`, `${g ? `${g.unit[0].toUpperCase()}${g.unit.slice(1)} start closed and the` : 'The'} list draws only what fits.`, [
    { id: 'find', label: n.find ?? 'Find' },
    DISMISS,
  ])
}

export function noticeView(n: LiveNotice, ctx: NoticeContext): NoticeView {
  if (n.kind === 'outside_save') return saveView(n.save, ctx)
  if (n.kind === 'connection') return connectionView(n, ctx)
  if (n.kind === 'size') return sizeView(n)
  const why = n.holder ? `${n.holder} is holding it` : n.why
  const title = `${n.actor} couldn’t ${n.verb} ${n.target}${why ? `: ${why}` : ''}.`
  const ask = n.ask !== undefined ? (n.ask?.name ?? null) : n.holder
  return view('warn', 'status', title, '', ask ? [{ id: 'ask', label: `Ask ${ask}` }, DISMISS] : [DISMISS])
}

/** The one notice to show: a connection first, else the newest of the rest
 *  that isn't dismissed. `notices` is oldest first. */
export function pickNotice({
  connection,
  notices,
  dismissed,
}: {
  connection: Extract<LiveNotice, { kind: 'connection' }> | null
  notices: readonly LiveNotice[]
  dismissed?: (n: LiveNotice) => boolean
}): LiveNotice | null {
  if (connection) return connection
  for (let i = notices.length - 1; i >= 0; i--) if (!dismissed?.(notices[i])) return notices[i]
  return null
}
