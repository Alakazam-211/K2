// Nav row status occupancy (plan-agent-status-unify-v1 S1–S3). The row's
// second line is always 14px and its status cluster is always mounted, so
// idle vs working never reflows the name row. And one renderer: every agent
// list draws activity through `AgentStatusCluster` — no list keeps its own
// spinner, squares or live-session dot.

import { describe, expect, it } from 'vitest'
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { dirname, join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'

const dir = dirname(fileURLToPath(import.meta.url))
const sidebarSrc = readFileSync(join(dir, 'Sidebar.tsx'), 'utf8')
const activeBarSrc = readFileSync(join(dir, 'ActiveBar.tsx'), 'utf8')
const tagsSrc = readFileSync(join(dir, 'NavProjectTags.tsx'), 'utf8')
const globalsSrc = readFileSync(join(dir, '../../globals.css'), 'utf8')

function sliceFn(src: string, name: string, until: string): string {
  const start = src.indexOf(`function ${name}`)
  if (start < 0) throw new Error(`missing function ${name}`)
  const end = src.indexOf(until, start + 1)
  if (end < 0) throw new Error(`missing terminator ${until} after ${name}`)
  return src.slice(start, end)
}

const agentRowStatus = sliceFn(sidebarSrc, 'AgentRowStatus', 'function NavWorktreeRow')
const singleProjectItem = sliceFn(sidebarSrc, 'SingleProjectItem', 'function WorkspaceButton')
// The two-row template lives in the shared row shell (Agents + Home rows).
const agentRowButton = sliceFn(sidebarSrc, 'AgentRowButton', 'function SingleProjectItem')
const activeBarItem = sliceFn(activeBarSrc, 'ActiveBarItem', 'export default function ActiveBar')

describe('nav row status: the shared cluster', () => {
  it('AgentRowStatus always renders the cluster (never null)', () => {
    expect(agentRowStatus).toContain('<AgentStatusCluster')
    expect(agentRowStatus).not.toMatch(/return null/)
    expect(agentRowStatus).toMatch(/React\.JSX\.Element\s*\{/)
  })

  it('the Agents row draws the cluster with its heartbeat and chord index', () => {
    expect(singleProjectItem).toContain('<AgentRowStatus')
    expect(singleProjectItem).toContain('heartbeatEnabled={project.heartbeatEnabled}')
    expect(singleProjectItem).toContain('index={shortcutIndex ?? null}')
    expect(singleProjectItem).not.toContain('braille-spinner')
  })

  it('the Active drawer row draws the same cluster, nothing of its own', () => {
    expect(activeBarItem).toContain('<AgentStatusCluster view={view} index={index} />')
    for (const gone of ['braille-spinner', 'ActivityMark', 'liveSessionCwds', 'IconAutonomous', 'status-warn-amber', 'shortcutNum']) {
      expect(activeBarItem).not.toContain(gone)
    }
  })
})

describe('SingleProjectItem reserved two-row template', () => {
  it('status row is always 14px even when empty — pin and unpin share this', () => {
    expect(agentRowButton).toContain('h-3.5 min-h-3.5 leading-none')
    expect(agentRowButton).not.toContain('-mt-px')
    expect(singleProjectItem).not.toContain('-mt-px')
    expect(singleProjectItem).toContain('<AgentRowButton')
  })

  it('name row uses leading-4 so it does not reflow when the spinner appears', () => {
    expect(agentRowButton).toContain('w-full leading-4')
    expect(agentRowButton).toContain('truncate flex-1 leading-4')
  })

  it('the right edge is the 12px inset: no -mr pull on line 1 (presence lines up with the number column)', () => {
    expect(agentRowButton).not.toContain('-mr-1')
  })
})

describe('NavProjectTags chips fit the 14px status slot', () => {
  it('tagBox is h-3.5 leading-none', () => {
    expect(tagsSrc).toMatch(/const tagBox =\s*'[^']*h-3\.5[^']*leading-none/)
    expect(tagsSrc).not.toMatch(/const tagBox =\s*'[^']*leading-3/)
  })
})

describe('.braille-spinner cannot change layout across frames (non-status uses)', () => {
  it('locks a 1em box with overflow hidden', () => {
    const block = globalsSrc.slice(globalsSrc.indexOf('.braille-spinner {'))
    const end = block.indexOf('.braille-spinner::after')
    if (end < 0) throw new Error('missing .braille-spinner element rule')
    const rule = block.slice(0, end)
    expect(rule).toMatch(/display:\s*inline-block/)
    expect(rule).toMatch(/width:\s*1em/)
    expect(rule).toMatch(/height:\s*1em/)
    expect(rule).toMatch(/line-height:\s*1/)
    expect(rule).toMatch(/overflow:\s*hidden/)
  })
})

describe('the working spinner stops for reduced motion', () => {
  it('one CSS rule stills every [data-agent-working] spinner', () => {
    const at = globalsSrc.indexOf('[data-agent-working] rect.e {')
    if (at < 0) throw new Error('missing the reduced-motion rule for [data-agent-working]')
    const media = globalsSrc.lastIndexOf('@media (prefers-reduced-motion: reduce)', at)
    expect(media).toBeGreaterThan(-1)
    expect(globalsSrc.slice(at, globalsSrc.indexOf('}', at))).toMatch(/animation:\s*none/)
  })

  it('the dirty-worktree pulse is gone from the nav', () => {
    expect(sidebarSrc).not.toContain('status-dot-dirty')
  })
})

// ── One renderer (§5): a source walk over components/** ───────────────────

const componentsDir = join(dir, '..')

function walk(d: string, out: string[] = []): string[] {
  for (const name of readdirSync(d)) {
    const p = join(d, name)
    if (statSync(p).isDirectory()) walk(p, out)
    else if (/\.tsx?$/.test(name) && !/\.test\.tsx?$/.test(name)) out.push(p)
  }
  return out
}

/** Surfaces part 2 (tabs, pinned tab, Thread) still converts. Shrink only:
 *  a new file here is a sixth look. */
const PART_2_PENDING = new Set([
  'Activity/ActivityMark.tsx',
  'RunningAgentsPanel/RunningAgentsPanel.tsx',
])

describe('one renderer for agent activity', () => {
  it('no component outside AgentStatus draws its own activity look', () => {
    const offenders: string[] = []
    for (const file of walk(componentsDir)) {
      const rel = relative(componentsDir, file)
      if (rel.startsWith('AgentStatus/') || PART_2_PENDING.has(rel)) continue
      const src = readFileSync(file, 'utf8')
      if (/<ActivityMark\b/.test(src)) offenders.push(`${rel}: ActivityMark`)
      if (src.includes('liveSessionCwds')) offenders.push(`${rel}: liveSessionCwds`)
      // A file that reads agent activity (the store, or a room's view of
      // it) never draws the braille spinner: working is the 3×3 grid.
      if (src.includes('braille-spinner') && /useActivity\(|stores\/activity'|\.activityView\b/.test(src)) {
        offenders.push(`${rel}: braille + activity`)
      }
    }
    expect(offenders).toEqual([])
  })

  it('the pending list only names files that still exist', () => {
    for (const rel of PART_2_PENDING) {
      expect(() => statSync(join(componentsDir, rel))).not.toThrow()
    }
  })
})
