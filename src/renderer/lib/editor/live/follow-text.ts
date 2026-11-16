// Follow on a live text (build-editors.md E4b; editors-notes.md §2 Follow, v2
// f10, v3.2 f09). The shell's follow state (lib/live-view/shared/follow.ts)
// says whom you follow, for the header control, the Here list and the say
// row; this keeps what only the text needs:
//   - your place when you started following (Back to my place);
//   - any scroll or typing of yours stops following;
//   - when the one you follow finishes (their hold ends, or an agent's edit
//     finishes while it holds nothing), following ends with "Atlas finished.
//     Stopped following.", Back to my place and Follow again.
// Pure: the editor dispatches and reads.

export type TextFollow =
  | { kind: 'idle' }
  | { kind: 'following'; who: string; home: number }
  | { kind: 'finished'; who: string; name: string; home: number }

export type TextFollowAction =
  | { type: 'follow'; who: string; home: number }
  | { type: 'stop' }
  /** You scrolled or typed. */
  | { type: 'moved' }
  | { type: 'finished'; who: string; name: string }
  | { type: 'back' }
  | { type: 'again'; home: number }
  /** The shell saw them leave (its own note says so). */
  | { type: 'gone'; who: string }

export const TEXT_FOLLOW_IDLE: TextFollow = { kind: 'idle' }

export function textFollowReducer(s: TextFollow, a: TextFollowAction): TextFollow {
  switch (a.type) {
    case 'follow':
      // Following someone else from following keeps your first place.
      return { kind: 'following', who: a.who, home: s.kind === 'following' ? s.home : a.home }
    case 'stop':
      return s.kind === 'idle' ? s : TEXT_FOLLOW_IDLE
    case 'moved':
      return s.kind === 'following' ? TEXT_FOLLOW_IDLE : s
    case 'finished':
      return s.kind === 'following' && s.who === a.who ? { kind: 'finished', who: s.who, name: a.name, home: s.home } : s
    case 'back':
      return s.kind === 'finished' ? TEXT_FOLLOW_IDLE : s
    case 'again':
      return s.kind === 'finished' ? { kind: 'following', who: s.who, home: a.home } : s
    case 'gone':
      return s.kind !== 'idle' && s.who === a.who ? TEXT_FOLLOW_IDLE : s
  }
}

export interface FinishedWords {
  text: string
  /** What a narrow header keeps: "Atlas finished." */
  short: string
  back: string
  backShort: string
  again: string
  againShort: string
}

/** The header's words once following ended because they finished (with the short forms a narrow header uses). */
export function finishedWords(s: TextFollow): FinishedWords | null {
  return s.kind === 'finished' ? { text: 'finished. Stopped following.', short: 'finished.', back: 'Back to my place', backShort: 'Back', again: 'Follow again', againShort: 'Follow' } : null
}
