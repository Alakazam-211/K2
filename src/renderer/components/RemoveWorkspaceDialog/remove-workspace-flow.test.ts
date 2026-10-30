import { describe, expect, it, vi } from 'vitest'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { runRemoval, type RemovalDeps } from './remove-workspace-flow'

function deps(over: Partial<RemovalDeps> = {}): RemovalDeps & { calls: string[] } {
  const calls: string[] = []
  return {
    calls,
    appIds: vi.fn(async (ws: string) => {
      calls.push(`appIds ${ws}`)
      return []
    }),
    uninstallApps: vi.fn(async (ws: string, drop: boolean) => {
      calls.push(`uninstall ${ws} drop=${drop}`)
    }),
    teardown: vi.fn(async () => {
      calls.push('teardown')
    }),
    remove: vi.fn(async (ws: string) => {
      calls.push(`remove ${ws}`)
    }),
    ...over,
  }
}

const lossRefusal = (): DaemonHttpError =>
  new DaemonHttpError('nothing was released…', 409, 'uninstall_drops_access', {
    code: 'uninstall_drops_access',
    released: [],
    apps: [{ app: 'jobs', wouldDrop: { logins: 40, spaces: 1 } }],
  })

describe('removing a workspace that maintains Apps (0.46.7)', () => {
  it('no Apps: teardown, then the removal', async () => {
    const d = deps()
    expect(await runRemoval('ws1', { uninstallApps: false, dropAccess: false }, d)).toEqual({ kind: 'done' })
    expect(d.calls).toEqual(['appIds ws1', 'teardown', 'remove ws1'])
  })

  it('Apps: stops before anything changes and lists them', async () => {
    const d = deps({ appIds: vi.fn(async () => ['jobs', 'crm']) })
    expect(await runRemoval('ws1', { uninstallApps: false, dropAccess: false }, d)).toEqual({ kind: 'apps', apps: ['jobs', 'crm'] })
    expect(d.teardown).not.toHaveBeenCalled()
    expect(d.remove).not.toHaveBeenCalled()
    expect(d.uninstallApps).not.toHaveBeenCalled()
  })

  it('uninstalling Apps that would drop access: lists the loss, nothing else happens', async () => {
    const d = deps({ uninstallApps: vi.fn(async () => Promise.reject(lossRefusal())) })
    expect(await runRemoval('ws1', { uninstallApps: true, dropAccess: false }, d)).toEqual({
      kind: 'loss',
      loss: [{ app: 'jobs', parts: ['40 App logins', '1 chat Space bound to it'] }],
    })
    expect(d.uninstallApps).toHaveBeenCalledWith('ws1', false)
    expect(d.teardown).not.toHaveBeenCalled()
    expect(d.remove).not.toHaveBeenCalled()
  })

  it('"Remove anyway": uninstalls with dropAccess, then teardown and removal', async () => {
    const d = deps()
    expect(await runRemoval('ws1', { uninstallApps: true, dropAccess: true }, d)).toEqual({ kind: 'done' })
    expect(d.calls).toEqual(['uninstall ws1 drop=true', 'teardown', 'remove ws1'])
  })

  it("a removal refused for Apps (the list wasn't readable) goes to the Apps step", async () => {
    const d = deps({
      appIds: vi.fn(async () => Promise.reject(new DaemonHttpError('Apps (beta) are off', 404, 'not_found', null))),
      remove: vi.fn(async () =>
        Promise.reject(
          new DaemonHttpError('Sales maintains an App…', 409, 'workspace_has_apps', {
            code: 'workspace_has_apps',
            apps: [{ app: 'jobs', state: 'running' }],
          }),
        ),
      ),
    })
    expect(await runRemoval('ws1', { uninstallApps: false, dropAccess: false }, d)).toEqual({ kind: 'apps', apps: ['jobs'] })
  })

  it('any other refusal is thrown (the dialog shows its message)', async () => {
    const poc = new DaemonHttpError('Sales is the Point of Contact for: Q4. Reassign the PoC first.', 409, 'poc_of_projects', null)
    const d = deps({ uninstallApps: vi.fn(async () => Promise.reject(poc)) })
    await expect(runRemoval('ws1', { uninstallApps: true, dropAccess: true }, d)).rejects.toBe(poc)
    expect(d.teardown).not.toHaveBeenCalled()
    expect(d.remove).not.toHaveBeenCalled()
  })
})
