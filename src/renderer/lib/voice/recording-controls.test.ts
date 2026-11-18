import { describe, expect, it } from 'vitest'
import type { NativeConsentPrompt, NativeRecording } from './native-voice-engine'
import { AGENT_READERS_LINE, formatRecordingClock, recordingBadgeTitle, recordingButtons, recordingNoticeText } from './recording-controls'

const rec = (over: Partial<NativeRecording> = {}): NativeRecording => ({
  recordingId: '42',
  myConsent: 'granted',
  recorderPresent: true,
  trigger: 'manual',
  required: false,
  startedBy: '2',
  prompt: null,
  ...over,
})
const prompt = (over: Partial<NativeConsentPrompt> = {}): NativeConsentPrompt => ({
  noticeVersion: 3141,
  startedByName: 'Alice',
  audioKeepDays: 30,
  required: false,
  appReaders: [],
  agentReaders: false,
  declineMode: 'not_join',
  ...over,
})

describe("Record and Stop: plugin 0.6.1's word only (Sew 0.9.2)", () => {
  const p = { recordCalls: true, manageChannels: true }

  it("the plugin's recording_controls decide, whatever K2 read of the person's permissions", () => {
    expect(recordingButtons({ perms: { recordCalls: false, manageChannels: false }, recording: null, controls: { canRecord: true, canStop: false } })).toEqual({ canRecord: true, canStop: false })
    expect(recordingButtons({ perms: p, recording: rec({ startedBy: '1' }), controls: { canRecord: false, canStop: false } })).toEqual({ canRecord: false, canStop: false })
    expect(recordingButtons({ perms: null, recording: rec(), controls: { canRecord: false, canStop: true } })).toEqual({ canRecord: false, canStop: true })
  })

  it("before any recording_controls, the recording event's can_record / can_stop (written only when true)", () => {
    expect(recordingButtons({ perms: p, recording: rec({ canStop: true }), controls: null })).toEqual({ canRecord: false, canStop: true })
    expect(recordingButtons({ perms: p, recording: rec({ canRecord: true }) })).toEqual({ canRecord: true, canStop: false })
  })

  it('no word from the plugin shows neither: K2 derives nothing from RECORD_CALLS, MANAGE_CHANNELS or the starter', () => {
    expect(recordingButtons({ perms: p, recording: null })).toEqual({ canRecord: false, canStop: false })
    expect(recordingButtons({ perms: p, recording: rec({ startedBy: '1' }), controls: null })).toEqual({ canRecord: false, canStop: false })
    expect(recordingButtons({ perms: null, recording: null, controls: null })).toEqual({ canRecord: false, canStop: false })
  })

  it('a Voice & Chat only login never records or stops, even if the plugin says it may', () => {
    const vco = { ...p, voiceChatOnly: true }
    expect(recordingButtons({ perms: vco, recording: null, controls: { canRecord: true, canStop: false } })).toEqual({ canRecord: false, canStop: false })
    expect(recordingButtons({ perms: vco, recording: rec({ startedBy: '1', canStop: true }) })).toEqual({ canRecord: false, canStop: false })
  })
})

describe("K2's own recording notice", () => {
  it('names who started it, what is kept and for how long, with OK and Leave, no opt-out', () => {
    const t = recordingNoticeText({ roomName: 'Standup', prompt: prompt(), agentReaders: false, ask: 'in_call' })
    expect(t.title).toBe('This call is being recorded')
    expect(t.lines[0]).toBe('Alice started recording this call.')
    expect(t.lines).toContain('The audio is kept for 30 days.')
    expect(t.lines.at(-1)).toBe('Staying in the call means being recorded.')
    expect([t.ok, t.leave]).toEqual(['OK', 'Leave call'])
    const all = [t.title, ...t.lines, t.ok, t.leave].join(' ')
    expect(all).not.toMatch(/without being recorded|stop recording me|don't record me/i)
    const j = recordingNoticeText({ roomName: 'Standup', prompt: prompt({ startedByName: null, required: true, audioKeepDays: 0 }), agentReaders: false, ask: 'join' })
    expect(j.title).toBe('Standup is being recorded')
    expect(j.lines[0]).toBe('This room records every call with more than one person.')
    expect(j.lines).toContain('The audio is deleted once the transcript is final.')
    expect([j.ok, j.leave]).toEqual(['OK', 'Don’t join'])
  })

  it('joining a room names the room, as the title does; in the call it is this call', () => {
    const j = recordingNoticeText({ roomName: 'Design review', prompt: prompt({ startedByName: 'Ana Ruiz' }), agentReaders: false, ask: 'join' })
    expect(j.title).toBe('Design review is being recorded')
    expect(j.lines[0]).toBe('Ana Ruiz started recording in Design review.')
    expect(j.lines.join(' ')).not.toMatch(/this call/)
    const t = recordingNoticeText({ roomName: 'Design review', prompt: prompt({ startedByName: 'Ana Ruiz' }), agentReaders: false, ask: 'in_call' })
    expect(t.lines[0]).toBe('Ana Ruiz started recording this call.')
  })

  it('every word people read uses curly apostrophes', () => {
    for (const dm of [false, true])
      for (const ask of ['join', 'in_call'] as const)
        for (const p of [prompt(), prompt({ startedByName: null, required: true }), prompt({ startedByName: null })]) {
          const t = recordingNoticeText({ roomName: 'Standup', prompt: p, agentReaders: true, ask, dm })
          expect([t.title, ...t.lines, t.ok, t.leave].join(' ')).not.toContain("'")
        }
    const room = recordingNoticeText({ roomName: 'Standup', prompt: prompt(), agentReaders: false, ask: 'join' })
    expect(room.lines).toContain('Everyone’s voice is recorded and transcribed. The transcript is kept in this room’s chat.')
  })

  it('says agents may read the transcript when an agent holds the switch (or Sew says so)', () => {
    expect(recordingNoticeText({ roomName: 'S', prompt: prompt(), agentReaders: false, ask: 'in_call' }).lines).not.toContain(AGENT_READERS_LINE)
    expect(recordingNoticeText({ roomName: 'S', prompt: prompt(), agentReaders: true, ask: 'in_call' }).lines).toContain('Agents on this server may read the transcript.')
    expect(recordingNoticeText({ roomName: 'S', prompt: prompt({ agentReaders: true }), agentReaders: false, ask: 'join' }).lines).toContain(AGENT_READERS_LINE)
  })

  it('the badge names what started it', () => {
    expect(recordingBadgeTitle(rec({ trigger: 'required' }), null)).toMatch(/every call with more than one person/)
    expect(recordingBadgeTitle(rec(), 'Alice')).toBe('Alice started recording')
    // With its start time: "Alice started recording at 2:41 PM".
    expect(recordingBadgeTitle(rec(), 'Alice', false, new Date(2026, 10, 17, 14, 41).getTime())).toMatch(/^Alice started recording at 2:41\sPM$/)
    expect(recordingBadgeTitle(rec({ trigger: 'auto' }), null, false, new Date(2026, 10, 17, 14, 41).getTime())).toMatch(/^This room started recording at 2:41\sPM: it records its calls$/)
    expect(recordingBadgeTitle(rec(), null)).toBe('Recording')
  })

  it('the badge clock: mm:ss, then h:mm:ss', () => {
    expect(formatRecordingClock(192_000)).toBe('03:12')
    expect(formatRecordingClock(0)).toBe('00:00')
    expect(formatRecordingClock(3_792_000)).toBe('1:03:12')
  })
})

describe("a DM call's recording notice (prd-home-call-recording-v1 §2, HR-8)", () => {
  it('says this DM, OK and Leave, and the PRD wording, line by line', () => {
    const t = recordingNoticeText({ roomName: 'Alex', prompt: prompt({ startedByName: 'Alex' }), agentReaders: false, ask: 'in_call', dm: true })
    expect(t.title).toBe('This call is being recorded')
    expect(t.lines).toEqual([
      'Alex started recording this call.',
      'Everyone’s voice is recorded and transcribed. The transcript is kept in this DM.',
      'The audio is kept for 30 days.',
      'Staying in the call means being recorded.',
    ])
    expect([t.ok, t.leave]).toEqual(['OK', 'Leave call'])
    expect([t.title, ...t.lines].join(' ')).not.toMatch(/room/i)
  })

  it("joining a DM call that already records: the same notice, Don't join", () => {
    const j = recordingNoticeText({ roomName: 'Alex', prompt: prompt(), agentReaders: false, ask: 'join', dm: true })
    expect(j.title).toBe('This call is being recorded')
    expect(j.lines.at(-1)).toBe('Joining means being recorded.')
    expect([j.ok, j.leave]).toEqual(['OK', 'Don’t join'])
  })

  it("never K2's agent line in a DM, even when an agent holds Read call transcripts; Sew's own word still shows", () => {
    expect(recordingNoticeText({ roomName: 'Alex', prompt: prompt(), agentReaders: true, ask: 'in_call', dm: true }).lines).not.toContain(AGENT_READERS_LINE)
    expect(recordingNoticeText({ roomName: 'Alex', prompt: prompt({ agentReaders: true }), agentReaders: false, ask: 'in_call', dm: true }).lines).toContain(AGENT_READERS_LINE)
  })

  it("a DM's badge names the server's rule, never a room", () => {
    expect(recordingBadgeTitle(rec({ trigger: 'required' }), null, true)).toBe('This server records every call with more than one person')
    expect(recordingBadgeTitle(rec(), 'Alex', true)).toBe('Alex started recording')
  })
})

describe('the audio keep days', () => {
  it('says nothing about the audio when the prompt has no keep days, never a guess', () => {
    const t = recordingNoticeText({ roomName: 'Standup', prompt: prompt({ audioKeepDays: null }), agentReaders: false, ask: 'in_call' })
    expect(t.lines.some((l) => /audio/i.test(l))).toBe(false)
  })
  it("uses the recording's own number", () => {
    const t = recordingNoticeText({ roomName: 'Standup', prompt: prompt({ audioKeepDays: 7 }), agentReaders: false, ask: 'in_call' })
    expect(t.lines).toContain('The audio is kept for 7 days.')
  })
})
