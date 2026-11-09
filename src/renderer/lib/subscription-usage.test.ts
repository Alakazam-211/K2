import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import {
  buttonChips,
  isChipWindow,
  buttonLabel,
  formatResetsIn,
  harnessName,
  isAirgapped,
  isSignedIn,
  parseSubscriptionDoc,
  percentUsed,
  PROBED_HARNESSES,
  visibleHarnesses,
  type SubscriptionDoc,
} from './subscription-usage'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..')

const MOUNTS = [
  'src/renderer/components/TopBar/TopBar.tsx',
  'src/renderer/components/Layout/FocusLayout.tsx',
  'src/renderer/components/Settings/Settings.tsx',
  'src/renderer/components/Projects/ProjectsPage.tsx',
  'src/renderer/components/Wiki/WikiPage.tsx',
  'src/renderer/components/Feedback/FeedbackPage.tsx',
]

function doc(partial: SubscriptionDoc): SubscriptionDoc {
  return partial
}

describe('subscription window math', () => {
  it('drives the Claude chip from the greater of Session and Weekly, never a model-scoped limit', () => {
    const chips = buttonChips(
      doc({
        harnesses: [
          {
            harness: 'claude',
            plan: 'Max 20x',
            windows: [
              { label: 'Session', used: 0.42, resetsAt: '2026-09-26T20:00:00Z' },
              { label: 'Weekly', used: 0.31, resetsAt: '2026-10-03T00:00:00Z' },
              { label: 'Fable Weekly', used: 0.97, resetsAt: '2026-10-03T00:00:00Z' },
            ],
            checkedAt: '2026-09-26T15:00:00Z',
            status: '',
          },
        ],
      }),
    )
    expect(chips).toEqual([{ harness: 'claude', used: 42 }])
  })

  it('turns weekly used 0.31 into 31% used and labels Claude', () => {
    expect(percentUsed(0.31)).toBe(31)
    const label = buttonLabel(
      doc({
        harnesses: [
          {
            harness: 'claude',
            plan: 'Max 20x',
            windows: [
              { label: 'Weekly', used: 0.31, resetsAt: '2026-10-03T00:00:00Z' },
              { label: 'Session', used: 0.18, resetsAt: '2026-09-26T20:00:00Z' },
            ],
            checkedAt: '2026-09-26T15:00:00Z',
            status: '',
          },
        ],
      }),
    )
    expect(label).toBe('Claude 31%')
  })

  it('picks the highest used signed-in window and hides an unprobed harness', () => {
    expect([...PROBED_HARNESSES]).toEqual(['claude', 'codex', 'grok'])
    expect(harnessName('grok')).toBe('Grok')
    const label = buttonLabel(
      doc({
        harnesses: [
          {
            harness: 'claude',
            plan: 'Max 20x',
            windows: [{ label: 'Weekly', used: 0.31, resetsAt: '2026-10-03T00:00:00Z' }],
            checkedAt: '2026-09-26T15:00:00Z',
            status: '',
          },
          {
            harness: 'codex',
            plan: 'plus',
            windows: [{ label: 'Weekly', used: 0.82, resetsAt: '2026-10-03T00:00:00Z' }],
            checkedAt: '2026-09-26T15:00:00Z',
            status: '',
          },
          {
            harness: 'grok',
            plan: 'SuperGrok',
            windows: [{ label: 'Weekly', used: 0.19, resetsAt: '2026-10-03T00:00:00Z' }],
            checkedAt: '2026-09-26T15:00:00Z',
            status: '',
          },
          {
            harness: 'gemini',
            plan: 'Ultra',
            windows: [{ label: 'Weekly', used: 0.99, resetsAt: '2026-10-03T00:00:00Z' }],
            checkedAt: '2026-09-26T15:00:00Z',
            status: '',
          },
        ],
      }),
    )
    expect(label).toBe('Claude 31% Codex 82% Grok 19%')
    const rows = visibleHarnesses(
      doc({
        harnesses: [
          {
            harness: 'grok',
            plan: '',
            windows: [],
            checkedAt: '',
            status: 'Not signed in',
          },
          {
            harness: 'gemini',
            plan: '',
            windows: [{ label: 'Weekly', used: 0.5, resetsAt: '2026-10-03T00:00:00Z' }],
            checkedAt: '',
            status: '',
          },
          {
            harness: 'claude',
            plan: '',
            windows: [],
            checkedAt: '',
            status: 'Not signed in',
          },
        ],
      }),
    )
    expect(rows.map((row) => row.harness)).toEqual(['grok', 'claude'])
  })

  it('reads Usage when nothing is signed in, not 0%', () => {
    const label = buttonLabel(
      doc({
        harnesses: [
          {
            harness: 'claude',
            plan: '',
            windows: [{ label: 'Weekly', used: 0, resetsAt: '2026-10-03T00:00:00Z' }],
            checkedAt: '2026-09-26T15:00:00Z',
            status: 'Not signed in',
          },
          {
            harness: 'codex',
            plan: '',
            windows: [],
            checkedAt: '2026-09-26T15:00:00Z',
            status: 'Sign-in expired',
          },
        ],
      }),
    )
    expect(label).toBe('Usage')
    expect(label).not.toContain('0%')
  })

  it('hides a blank probe and keeps a signed-in row with no meter', () => {
    expect(
      isSignedIn({
        harness: 'codex',
        plan: '',
        windows: [],
        checkedAt: '',
        status: '',
      }),
    ).toBe(false)
    expect(
      isSignedIn({
        harness: 'claude',
        plan: 'Max',
        windows: [],
        checkedAt: '',
        status: 'No usage window',
      }),
    ).toBe(true)
  })

  it('parses the single-record fields and leaves them absent on an older daemon', () => {
    const doc = parseSubscriptionDoc({
      harnesses: [
        {
          harness: 'claude',
          plan: 'Max',
          windows: [{ label: 'Weekly', used: 0.09, resetsAt: '', id: 'seven_day', observedAt: '2026-11-09T11:58:00Z', source: 'probe' }],
          checkedAt: '2026-11-09T11:58:00Z',
          status: '',
          maxPercent: 9,
          chipWindow: { id: 'seven_day', label: 'Weekly', percent: 9, observedAt: '2026-11-09T11:58:00Z' },
          asOf: '2026-11-09T11:58:00Z',
          accountId: 'acct-1',
        },
        { harness: 'codex', plan: '', windows: [], checkedAt: '', status: '' },
      ],
    })
    const [claude, codex] = doc.harnesses
    expect(claude.chipWindow).toEqual({ id: 'seven_day', label: 'Weekly', percent: 9, observedAt: '2026-11-09T11:58:00Z' })
    expect(claude.asOf).toBe('2026-11-09T11:58:00Z')
    expect(claude.accountId).toBe('acct-1')
    expect(claude.windows[0].observedAt).toBe('2026-11-09T11:58:00Z')
    expect(claude.windows[0].source).toBe('probe')
    expect(codex.chipWindow).toBeUndefined()
    expect(codex.asOf).toBeUndefined()
  })

  it('names the chip window for one bar only', () => {
    const w = { label: 'Weekly', used: 0.1, resetsAt: '', id: 'seven_day' }
    expect(isChipWindow({ id: 'seven_day', label: 'Weekly', percent: 10, observedAt: null }, w)).toBe(true)
    expect(isChipWindow({ id: 'five_hour', label: 'Session', percent: 10, observedAt: null }, w)).toBe(false)
    expect(isChipWindow(null, w)).toBe(false)
    expect(isChipWindow(undefined, w)).toBe(false)
    expect(isChipWindow({ id: '', label: 'Weekly', percent: 10, observedAt: null }, { ...w, id: undefined })).toBe(true)
  })

  it('formats time until reset', () => {
    const now = Date.parse('2026-09-26T12:00:00Z')
    expect(formatResetsIn('2026-09-26T15:00:00Z', now)).toBe('3h')
    expect(formatResetsIn('2026-09-30T12:00:00Z', now)).toBe('4d')
  })
})

describe('UsageButton mounts', () => {
  it('sits left of TimerButton on each of the six bars', () => {
    const utils = readFileSync(
      resolve(root, 'src/renderer/components/TopBar/TopBarUtilities.tsx'),
      'utf8',
    )
    const usage = utils.indexOf('<UsageButton />')
    const timer = utils.indexOf('<TimerButton />')
    expect(usage).toBeGreaterThanOrEqual(0)
    expect(timer).toBeGreaterThan(usage)
    expect(utils.slice(usage, timer).replace(/\s+/g, '')).toBe('<UsageButton/><TopBarPipe/>')
    for (const rel of MOUNTS) {
      const text = readFileSync(resolve(root, rel), 'utf8')
      expect(text.includes('<TopBarUtilities'), rel).toBe(true)
      expect(text.includes('<UsageButton />'), rel).toBe(false)
      expect(text.includes('<TimerButton />'), rel).toBe(false)
    }
  })

  it('is not inside TimerButton, DesktopChromeRight, or GateChrome', () => {
    const timer = readFileSync(
      resolve(root, 'src/renderer/components/Timer/TimerButton.tsx'),
      'utf8',
    )
    expect(timer.includes('UsageButton')).toBe(false)
    expect(timer.includes('subscription-usage')).toBe(false)
    expect(timer.includes('if (!visible) return null')).toBe(true)
    const chrome = readFileSync(
      resolve(root, 'src/renderer/components/TopBar/DesktopChromeRight.tsx'),
      'utf8',
    )
    expect(chrome.includes('UsageButton')).toBe(false)
    expect(chrome.includes('subscription-usage')).toBe(false)
    const gate = readFileSync(
      resolve(root, 'src/renderer/components/TopBar/GateChrome.tsx'),
      'utf8',
    )
    expect(gate.includes('UsageButton')).toBe(false)
    expect(gate.includes('TimerButton')).toBe(false)
    expect(gate.includes('subscription-usage')).toBe(false)
  })
})

describe('parseSubscriptionDoc', () => {
  const EMPTY: SubscriptionDoc = { harnesses: [] }

  it('turns anything that is not the daemon shape into no harnesses', () => {
    for (const raw of [
      undefined,
      null,
      '',
      'not json',
      42,
      [],
      [{ harness: 'claude' }],
      {},
      { error: 'role_required', required: 'member', role: 'viewer' },
      { harnesses: {} },
      { harnesses: null },
      { harnesses: 'claude' },
      { keepAwake: { mode: 'off' } },
    ]) {
      expect(parseSubscriptionDoc(raw)).toEqual(EMPTY)
    }
  })

  it('keeps the daemon air-gap flag and drops every chip for it', () => {
    const row = { harness: 'claude', plan: '', windows: [], checkedAt: '2026-10-05T00:00:00Z', status: 'Off in air-gap mode' }
    const got = parseSubscriptionDoc({ airgap: true, reason: 'Off in air-gap mode', harnesses: [row] })
    expect(got).toEqual({ airgap: true, harnesses: [row] })
    expect(isAirgapped(got)).toBe(true)
    expect(buttonChips(got)).toEqual([])
    expect(isAirgapped(parseSubscriptionDoc({ airgap: 'yes', harnesses: [row] }))).toBe(false)
    expect('airgap' in parseSubscriptionDoc({ harnesses: [row] })).toBe(false)
  })

  it('keeps a well-formed doc as it is', () => {
    const doc: SubscriptionDoc = {
      harnesses: [
        {
          harness: 'claude',
          plan: 'Max',
          windows: [{ label: 'Weekly', used: 0.31, resetsAt: '2026-10-05T16:00:00Z' }],
          checkedAt: '2026-10-03T21:05:27Z',
          status: '',
        },
        { harness: 'codex', plan: '', windows: [], checkedAt: '2026-10-03T21:05:27Z', status: 'Not signed in' },
      ],
    }
    expect(parseSubscriptionDoc(JSON.parse(JSON.stringify(doc)))).toEqual(doc)
  })

  it('coerces each row: windows to a list, missing strings to empty, bad rows and windows dropped', () => {
    const got = parseSubscriptionDoc({
      harnesses: [
        { harness: 'claude', windows: {}, plan: 7 },
        { harness: 'codex', windows: [null, { label: 'Week' }, { label: 'Weekly', used: 0.5 }] },
        { plan: 'no harness name' },
        null,
        'grok',
      ],
    })
    expect(got).toEqual({
      harnesses: [
        { harness: 'claude', plan: '', windows: [], checkedAt: '', status: '' },
        {
          harness: 'codex',
          plan: '',
          windows: [{ label: 'Weekly', used: 0.5, resetsAt: '' }],
          checkedAt: '',
          status: '',
        },
      ],
    })
  })

  it('helpers tolerate a doc without a harness list', () => {
    const broken = {} as unknown as SubscriptionDoc
    expect(visibleHarnesses(broken)).toEqual([])
    expect(visibleHarnesses(null)).toEqual([])
    expect(buttonChips(broken)).toEqual([])
    expect(buttonLabel(broken)).toBe('Usage')
  })
})
