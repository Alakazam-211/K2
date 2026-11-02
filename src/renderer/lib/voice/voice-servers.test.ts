import { describe, expect, it } from 'vitest'
import type { HostEntry } from '@/lib/host-pool'
import { anyVoiceServer, lastSeenText, voiceServers } from './voice-servers'

function entry(hostKey: string, patch: Partial<HostEntry> & { features?: string[] } = {}): HostEntry {
  const { features = ['voice-rooms-v1'], ...rest } = patch
  return {
    hostKey,
    saved: true,
    hostId: hostKey,
    reach: 'live',
    boot: { phase: 'ready', ready: true, version: '0.46.8', protocol: 1, instanceId: 'i', features, view: 'full', at: 1000 },
    auth: 'ok',
    authNote: null,
    role: 'member',
    presence: null,
    offlineStreak: 0,
    checkedAt: 1000,
    ...rest,
  }
}

const labels: Record<string, string> = { 'sewlab.k2.dev': 'sewlab', 'alpha.k2.dev': 'Alpha', 'zed.k2.dev': 'zed', local: 'This computer' }
const labelOf = (k: string): string => labels[k] ?? k

describe('voiceServers', () => {
  it('lists signed-in servers that report rooms: the window first, then A to Z', () => {
    const out = voiceServers({
      keys: ['zed.k2.dev', 'local', 'sewlab.k2.dev', 'alpha.k2.dev'],
      windowKey: 'zed.k2.dev',
      entries: {
        'zed.k2.dev': entry('zed.k2.dev', { auth: 'signin-required' }), // the window's own: its login is the window's
        local: entry('local', { features: [] }), // a Mac: no rooms
        'sewlab.k2.dev': entry('sewlab.k2.dev'),
        'alpha.k2.dev': entry('alpha.k2.dev'),
      },
      labelOf,
      reportedBefore: new Set(),
    })
    expect(out.map((s) => [s.label, s.kind, s.isWindow])).toEqual([
      ['zed', 'live', true],
      ['Alpha', 'live', false],
      ['sewlab', 'live', false],
    ])
    expect(anyVoiceServer(out)).toBe(true)
  })

  it('a server without the feature (preview off) is never listed, and the button hides', () => {
    const out = voiceServers({
      keys: ['sewlab.k2.dev'],
      windowKey: 'local',
      entries: { 'sewlab.k2.dev': entry('sewlab.k2.dev', { features: [] }) },
      labelOf,
      reportedBefore: new Set(),
    })
    expect(out).toEqual([])
    expect(anyVoiceServer(out)).toBe(false)
  })

  it('signed out: a row only for a server that had rooms before (§4.3)', () => {
    const entries = { 'sewlab.k2.dev': entry('sewlab.k2.dev', { auth: 'kicked', authNote: 'Removed from sewlab. Sign in again.' }) }
    const before = voiceServers({ keys: ['sewlab.k2.dev'], windowKey: 'local', entries, labelOf, reportedBefore: new Set(['sewlab.k2.dev']) })
    expect(before.map((s) => [s.kind, s.authNote])).toEqual([['signed-out', 'Removed from sewlab. Sign in again.']])
    const never = voiceServers({ keys: ['sewlab.k2.dev'], windowKey: 'local', entries, labelOf, reportedBefore: new Set() })
    expect(never).toEqual([])
    expect(anyVoiceServer(before)).toBe(false)
  })

  it('a role below Member is left out, never shown as signed out', () => {
    const out = voiceServers({
      keys: ['sewlab.k2.dev'],
      windowKey: 'local',
      entries: { 'sewlab.k2.dev': entry('sewlab.k2.dev', { role: 'viewer' }) },
      labelOf,
      reportedBefore: new Set(['sewlab.k2.dev']),
    })
    expect(out).toEqual([])
  })

  it('offline: greyed with last seen, only when it had rooms', () => {
    const out = voiceServers({
      keys: ['sewlab.k2.dev'],
      windowKey: 'local',
      entries: { 'sewlab.k2.dev': entry('sewlab.k2.dev', { reach: 'offline' }) },
      labelOf,
      reportedBefore: new Set(),
    })
    expect(out.map((s) => [s.kind, s.lastSeenAt])).toEqual([['offline', 1000]])
    expect(lastSeenText(1000, 1000 + 3 * 60_000)).toBe('Offline · last seen 3 min ago')
  })

  it('one server reached by two spellings is listed once', () => {
    const out = voiceServers({
      keys: ['sewlab.k2.dev', 'https://SEWLAB.k2.dev/'],
      windowKey: 'local',
      entries: { 'sewlab.k2.dev': entry('sewlab.k2.dev') },
      labelOf,
      reportedBefore: new Set(),
    })
    expect(out).toHaveLength(1)
  })
})
