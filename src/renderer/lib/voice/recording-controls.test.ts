import { describe, expect, it } from 'vitest'
import type { NativeConsentPrompt, NativeRecording } from './native-voice-engine'
import { AGENT_READERS_LINE, recordingBadgeTitle, recordingButtons, recordingNoticeText } from './recording-controls'

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
    expect([j.ok, j.leave]).toEqual(['OK', "Don't join"])
  })

  it('says agents may read the transcript when an agent holds the switch (or Sew says so)', () => {
    expect(recordingNoticeText({ roomName: 'S', prompt: prompt(), agentReaders: false, ask: 'in_call' }).lines).not.toContain(AGENT_READERS_LINE)
    expect(recordingNoticeText({ roomName: 'S', prompt: prompt(), agentReaders: true, ask: 'in_call' }).lines).toContain('Agents on this server may read the transcript.')
    expect(recordingNoticeText({ roomName: 'S', prompt: prompt({ agentReaders: true }), agentReaders: false, ask: 'join' }).lines).toContain(AGENT_READERS_LINE)
  })

  it('the badge names what started it', () => {
    expect(recordingBadgeTitle(rec({ trigger: 'required' }), null)).toMatch(/every call with more than one person/)
    expect(recordingBadgeTitle(rec(), 'Alice')).toBe('Recording: started by Alice')
  })
})

describe("a DM call's recording notice (prd-home-call-recording-v1 §2, HR-8)", () => {
  it('says this DM, OK and Leave, and the PRD wording, line by line', () => {
    const t = recordingNoticeText({ roomName: 'Alex', prompt: prompt({ startedByName: 'Alex' }), agentReaders: false, ask: 'in_call', dm: true })
    expect(t.title).toBe('This call is being recorded')
    expect(t.lines).toEqual([
      'Alex started recording this call.',
      "Everyone's voice is recorded and transcribed. The transcript is kept in this DM.",
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
    expect([j.ok, j.leave]).toEqual(['OK', "Don't join"])
  })

  it("never K2's agent line in a DM, even when an agent holds Read call transcripts; Sew's own word still shows", () => {
    expect(recordingNoticeText({ roomName: 'Alex', prompt: prompt(), agentReaders: true, ask: 'in_call', dm: true }).lines).not.toContain(AGENT_READERS_LINE)
    expect(recordingNoticeText({ roomName: 'Alex', prompt: prompt({ agentReaders: true }), agentReaders: false, ask: 'in_call', dm: true }).lines).toContain(AGENT_READERS_LINE)
  })

  it("a DM's badge names the server's rule, never a room", () => {
    expect(recordingBadgeTitle(rec({ trigger: 'required' }), null, true)).toBe('Recording: this server records every call with more than one person')
    expect(recordingBadgeTitle(rec(), 'Alex', true)).toBe('Recording: started by Alex')
  })
})
