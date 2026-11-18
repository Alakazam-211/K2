// Why a transcript failed, as Sew says it: each known code in K2's words,
// an unknown code in Sew's own message, an empty message and no field at
// all keep the plain line, and a partial-only code is a note, never a
// failure. No words point to the audio.

import { describe, expect, it } from 'vitest'
import { PARTIAL_NOT_CAPTURED_LINE, parseTranscriptFailure, transcriptFailureWords, transcriptPartialNote } from './transcript-failure'

describe("Sew's transcript failure", () => {
  it('reads {code, message}; anything else is no reason', () => {
    expect(parseTranscriptFailure({ code: 'audio_gone', message: 'Audio purged.' })).toEqual({ code: 'audio_gone', message: 'Audio purged.' })
    expect(parseTranscriptFailure({ code: 'x' })).toEqual({ code: 'x', message: '' })
    for (const v of [undefined, null, 'audio_gone', 3, [], {}, { code: 4, message: null }, { code: '  ', message: '' }]) {
      expect(parseTranscriptFailure(v)).toBeNull()
    }
  })

  it('each known code in our words', () => {
    expect(transcriptFailureWords({ code: 'audio_gone', message: 'ignored' })).toBe('the audio was removed before it could be transcribed.')
    expect(transcriptFailureWords({ code: 'unsupported_language', message: '' })).toBe('this language isn’t supported yet.')
    // Final: Sew already retried, so no retry and no "try again later".
    expect(transcriptFailureWords({ code: 'transcriber_error', message: 'Try again later' })).toBe('it couldn’t be made from this recording.')
  })

  it("an unknown code shows Sew's message; an empty one keeps the plain line", () => {
    expect(transcriptFailureWords({ code: 'quota_exceeded', message: 'the server ran out of transcription time.' })).toBe('the server ran out of transcription time.')
    expect(transcriptFailureWords({ code: 'quota_exceeded', message: '' })).toBeNull()
    expect(transcriptFailureWords(null)).toBeNull()
  })

  it('audio_not_captured is a note on a Ready card, never a failure', () => {
    const f = { code: 'audio_not_captured', message: 'Some segments were lost.' }
    expect(transcriptFailureWords(f)).toBeNull()
    expect(transcriptPartialNote(f)).toBe(PARTIAL_NOT_CAPTURED_LINE)
    expect(PARTIAL_NOT_CAPTURED_LINE).toBe('Part of the call wasn’t captured, so this transcript is incomplete.')
    expect(transcriptPartialNote({ code: 'audio_gone', message: '' })).toBeNull()
    expect(transcriptPartialNote(null)).toBeNull()
  })

  it('no words promise the audio', () => {
    for (const code of ['audio_gone', 'unsupported_language', 'transcriber_error']) {
      expect(transcriptFailureWords({ code, message: '' })).not.toMatch(/listen|play|download|try again/i)
    }
    expect(PARTIAL_NOT_CAPTURED_LINE).not.toMatch(/listen|play|download/i)
  })
})
