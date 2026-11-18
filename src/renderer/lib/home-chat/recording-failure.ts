// Why a recording's transcript failed, or why it has gaps, as Sew says it
// (Sew 0.10.5): `transcript_failure: {code, message}` beside the recording,
// on the record message (`Message.transcript_failure`) and on the voice
// rooms' recordings list (the daemon hands it on as `transcriptFailure`).
// An older Sew sends neither, and every card then reads as it did.
//
// The card's Failed state says "No transcript: <words>". The words never
// point to the audio: K2 has no playback or download. A transcriber error
// is final (Sew has already retried), so nothing here says "try again".

/** Sew's reason, as it came. */
export interface TranscriptFailure {
  code: string
  /** Sew's own words; may be empty. */
  message: string
}

/** Sew's `{code, message}`, or null for anything else (no field, an older
 *  Sew, a shape this client doesn't know). */
export function parseTranscriptFailure(v: unknown): TranscriptFailure | null {
  if (!v || typeof v !== 'object' || Array.isArray(v)) return null
  const o = v as Record<string, unknown>
  const code = typeof o.code === 'string' ? o.code.trim() : ''
  const message = typeof o.message === 'string' ? o.message.trim() : ''
  if (!code && !message) return null
  return { code, message }
}

/** Not a failure: Sew made the transcript, but part of the call never
 *  reached it. Shown as a note on a Ready card. */
export const AUDIO_NOT_CAPTURED = 'audio_not_captured'

/** The Ready card's note for `audio_not_captured`. */
export const PARTIAL_NOT_CAPTURED_LINE = 'Part of the call wasn’t captured, so this transcript is incomplete.'

const FAILURE_WORDS: Record<string, string> = {
  audio_gone: 'the audio was removed before it could be transcribed.',
  unsupported_language: 'this language isn’t supported yet.',
  transcriber_error: 'it couldn’t be made from this recording.',
}

/** The words after "No transcript: ", or null when there are none (no
 *  reason, a partial-only code, an unknown code with an empty message): the
 *  card then keeps its plain line. */
export function transcriptFailureWords(f: TranscriptFailure | null): string | null {
  if (!f || f.code === AUDIO_NOT_CAPTURED) return null
  return FAILURE_WORDS[f.code] ?? (f.message || null)
}

/** The Ready card's note, when Sew says part of the call wasn't captured. */
export function transcriptPartialNote(f: TranscriptFailure | null): string | null {
  return f?.code === AUDIO_NOT_CAPTURED ? PARTIAL_NOT_CAPTURED_LINE : null
}
