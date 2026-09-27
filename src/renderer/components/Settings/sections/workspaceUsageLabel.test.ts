import { describe, expect, it } from 'vitest'
import { OUTSIDE_WORKSPACES, workspaceUsageLabel, type UsageNameProject } from './workspaceUsageLabel'

const projects: UsageNameProject[] = [
  {
    name: 'Solo',
    path: '/solo',
    workspaces: [{ name: 'other', worktreePath: '/solo-wt' }],
  },
  {
    name: 'Proj',
    path: '/repo',
    workspaces: [
      { name: 'Feature', worktreePath: '/repo' },
      { name: '', worktreePath: '/repo-blank' },
      { name: 'Slash', worktreePath: '/slash-wt/' },
    ],
  },
  {
    name: 'Tmp',
    path: '/tmp/x',
    workspaces: [{ name: 'VarWs', worktreePath: '/var/y' }],
  },
  {
    name: 'StoredPrivate',
    path: '/elsewhere',
    workspaces: [{ name: 'PrivVar', worktreePath: '/private/var/z' }],
  },
]

function label(path: string, outside = false) {
  return workspaceUsageLabel({ path, outside }, projects)
}

describe('workspaceUsageLabel', () => {
  it('uses the project name for a project path', () => {
    expect(label('/solo')).toEqual({ name: 'Solo', subtitle: '/solo' })
  })

  it('prefers a worktree name over the project path', () => {
    expect(label('/repo')).toEqual({ name: 'Feature', subtitle: '/repo' })
    expect(label('/solo-wt')).toEqual({ name: 'other', subtitle: '/solo-wt' })
  })

  it('uses the project name when the worktree name is empty', () => {
    expect(label('/repo-blank')).toEqual({ name: 'Proj', subtitle: '/repo-blank' })
  })

  it('matches one trailing slash and keeps the original path', () => {
    expect(label('/solo/')).toEqual({ name: 'Solo', subtitle: '/solo/' })
    expect(label('/slash-wt')).toEqual({ name: 'Slash', subtitle: '/slash-wt' })
  })

  it('treats a /private prefix as the same /tmp or /var path', () => {
    expect(label('/private/tmp/x')).toEqual({ name: 'Tmp', subtitle: '/private/tmp/x' })
    expect(label('/tmp/x/')).toEqual({ name: 'Tmp', subtitle: '/tmp/x/' })
    expect(label('/private/var/y')).toEqual({ name: 'VarWs', subtitle: '/private/var/y' })
    expect(label('/var/z')).toEqual({ name: 'PrivVar', subtitle: '/var/z' })
  })

  it('uses the last path segment when nothing matches', () => {
    expect(label('/no/such/place')).toEqual({ name: 'place', subtitle: '/no/such/place' })
    expect(label('/no/such/place/')).toEqual({ name: 'place', subtitle: '/no/such/place/' })
  })

  it('keeps Outside workspaces and has no subtitle', () => {
    expect(label('/solo', true)).toEqual({ name: OUTSIDE_WORKSPACES, subtitle: null })
    expect(workspaceUsageLabel({ path: '', outside: true }, projects).subtitle).toBeNull()
  })
})
