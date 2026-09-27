import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import {
  buttonLabel,
  formatResetsIn,
  harnessName,
  isSignedIn,
  isStale,
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

  it('treats a cache older than 15 seconds as stale', () => {
    const now = Date.parse('2026-09-26T15:00:15Z')
    const fresh = '2026-09-26T15:00:01Z'
    const stale = '2026-09-26T14:59:00Z'
    expect(
      isStale(
        doc({
          harnesses: [
            { harness: 'claude', plan: '', windows: [], checkedAt: fresh, status: '' },
            { harness: 'codex', plan: '', windows: [], checkedAt: fresh, status: '' },
            { harness: 'grok', plan: '', windows: [], checkedAt: fresh, status: '' },
          ],
        }),
        now,
      ),
    ).toBe(false)
    expect(
      isStale(
        doc({
          harnesses: [
            { harness: 'claude', plan: '', windows: [], checkedAt: stale, status: '' },
            { harness: 'codex', plan: '', windows: [], checkedAt: fresh, status: '' },
            { harness: 'grok', plan: '', windows: [], checkedAt: fresh, status: '' },
          ],
        }),
        now,
      ),
    ).toBe(true)
    expect(
      isStale(
        doc({
          harnesses: [
            { harness: 'claude', plan: '', windows: [], checkedAt: fresh, status: '' },
            { harness: 'codex', plan: '', windows: [], checkedAt: fresh, status: '' },
          ],
        }),
        now,
      ),
    ).toBe(true)
    expect(isStale(null, now)).toBe(true)
  })

  it('formats time until reset', () => {
    const now = Date.parse('2026-09-26T12:00:00Z')
    expect(formatResetsIn('2026-09-26T15:00:00Z', now)).toBe('3h')
    expect(formatResetsIn('2026-09-30T12:00:00Z', now)).toBe('4d')
  })
})

describe('UsageButton mounts', () => {
  it('sits immediately left of TimerButton on each of the six bars', () => {
    for (const rel of MOUNTS) {
      const text = readFileSync(resolve(root, rel), 'utf8')
      const usageCount = text.split('<UsageButton />').length - 1
      const timerCount = text.split('<TimerButton />').length - 1
      expect(usageCount, rel).toBe(1)
      expect(timerCount, rel).toBe(1)
      const usage = text.indexOf('<UsageButton />')
      const timer = text.indexOf('<TimerButton />')
      expect(usage, rel).toBeGreaterThanOrEqual(0)
      expect(timer, rel).toBeGreaterThan(usage)
      expect(text.slice(usage, timer).replace(/\s+/g, ''), rel).toBe('<UsageButton/>')
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
