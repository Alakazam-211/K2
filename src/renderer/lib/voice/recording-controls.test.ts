import { describe, expect, it } from 'vitest'
import type { NativeConsentPrompt, NativeParticipant, NativeRecording } from './native-voice-engine'
import { AGENT_READERS_LINE, recordingBadgeTitle, recordingButtons, recordingNoticeText } from './recording-controls'

const me: NativeParticipant = { memberId: '1', isSelf: true, muted: false, deafened: false, serverMuted: false, serverDeafened: false, speakDenied: false }
const other: NativeParticipant = { ...me, memberId: '2', isSelf: false }
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

describe('Record and Stop, derived until Sew 0.9.2 sends can_record / can_stop', () => {
  it('Record needs RECORD_CALLS and no recording on', () => {
    expect(recordingButtons({ perms: { recordCalls: true, manageChannels: false }, recording: null, people: [me] })).toEqual({ canRecord: true, canStop: false })
    expect(recordingButtons({ perms: { recordCalls: false, manageChannels: true }, recording: null, people: [me] }).canRecord).toBe(false)
    expect(recordingButtons({ perms: { recordCalls: true, manageChannels: true }, recording: rec(), people: [me] }).canRecord).toBe(false)
    expect(recordingButtons({ perms: null, recording: null, people: [me] })).toEqual({ canRecord: false, canStop: false })
  })

  it('Stop: the starter, a DM, or MANAGE_CHANNELS', () => {
    const p = { recordCalls: true, manageChannels: false }
    expect(recordingButtons({ perms: p, recording: rec({ startedBy: '1' }), people: [me, other] }).canStop).toBe(true)
    expect(recordingButtons({ perms: p, recording: rec(), people: [me, other] }).canStop).toBe(false)
    expect(recordingButtons({ perms: p, recording: rec(), people: [me, other], isDm: true }).canStop).toBe(true)
    expect(recordingButtons({ perms: { ...p, manageChannels: true }, recording: rec(), people: [me, other] }).canStop).toBe(true)
  })

  it('Stop is hidden while a required room has two or more people', () => {
    const p = { recordCalls: true, manageChannels: true }
    expect(recordingButtons({ perms: p, recording: rec({ required: true }), people: [me, other] }).canStop).toBe(false)
    expect(recordingButtons({ perms: p, recording: rec({ required: true }), people: [me] }).canStop).toBe(true)
  })

  it("Sew 0.9.2's can_record / can_stop decide when the event has them; the derived rule is the fallback", () => {
    const p = { recordCalls: false, manageChannels: false }
    expect(recordingButtons({ perms: p, recording: rec({ canRecord: true, canStop: true }), people: [me, other] })).toEqual({ canRecord: true, canStop: true })
    expect(recordingButtons({ perms: { recordCalls: true, manageChannels: true }, recording: rec({ startedBy: '1', canRecord: false, canStop: false }), people: [me] })).toEqual({
      canRecord: false,
      canStop: false,
    })
    // Absent: derived from the perms (here: not the starter, no MANAGE_CHANNELS).
    expect(recordingButtons({ perms: p, recording: rec(), people: [me, other] })).toEqual({ canRecord: false, canStop: false })
  })

  it('a Voice & Chat only login never records or stops', () => {
    const p = { recordCalls: true, manageChannels: true, voiceChatOnly: true }
    expect(recordingButtons({ perms: p, recording: null, people: [me] })).toEqual({ canRecord: false, canStop: false })
    expect(recordingButtons({ perms: p, recording: rec({ startedBy: '1' }), people: [me] })).toEqual({ canRecord: false, canStop: false })
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
