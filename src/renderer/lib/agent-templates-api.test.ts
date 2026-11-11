// The agent-templates client (prd-agent-templates-v1; the T5–T7 contract):
// the routes and bodies it sends, an older server's plain 404 becoming
// "can't yet" (a coded 404 stays a real answer), the picker's order, and
// each refusal code's words.

import { beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({ get: vi.fn(), post: vi.fn(), postQuery: vi.fn() }))

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...a: unknown[]) => h.get(...a)),
    daemonCliPost: primaryOnly((...a: unknown[]) => h.post(...a)),
    daemonCliPostQuery: primaryOnly((...a: unknown[]) => h.postQuery(...a)),
  }
})

import {
  CREATE_UNSUPPORTED_TEXT,
  SAVE_ERROR_COPY,
  SAVE_UNSUPPORTED_TEXT,
  TemplatesUnsupportedError,
  authorLabel,
  hireErrorView,
  isMissingRoute,
  listTemplates,
  pickerGroups,
  saveErrorView,
  saveTemplate,
  secretFoundText,
  showTemplate,
  type TemplateSummary,
} from './agent-templates-api'
import { DaemonHttpError } from './daemon-http-error'
import { primaryScope } from '@/kessel/server-scope'

function t(id: string, over: Partial<TemplateSummary> = {}): TemplateSummary {
  return { id, version: 1, name: id, summary: '', kind: 'agent', aliases: [], author: 'K2', origin: 'builtin', hash: '', ...over }
}

beforeEach(() => {
  h.get.mockReset()
  h.post.mockReset()
  h.postQuery.mockReset()
})

describe('picker', () => {
  it('hides system templates, puts General first, and the library under Yours', () => {
    const g = pickerGroups([
      t('lead'),
      t('k5', { kind: 'system' }),
      t('general'),
      t('mine', { origin: 'library', author: 'user:rosson' }),
      t('chief-of-staff', { kind: 'system' }),
      t('writer'),
    ])
    expect(g.builtIn.map((x) => x.id)).toEqual(['general', 'lead', 'writer'])
    expect(g.yours.map((x) => x.id)).toEqual(['mine'])
  })

  it('names authors plainly', () => {
    expect(authorLabel('user:rosson')).toBe('rosson')
    expect(authorLabel('agent:scout')).toBe('the agent scout')
    expect(authorLabel('K2')).toBe('K2')
  })
})

describe('routes', () => {
  it('list and show read the template routes', async () => {
    h.get.mockResolvedValueOnce({ ok: true, templates: [], problems: [] })
    await listTemplates(primaryScope())
    expect(h.get).toHaveBeenLastCalledWith('agent-templates')
    h.get.mockResolvedValueOnce({ ok: true, template: { id: 'writer' } })
    expect((await showTemplate(primaryScope(), 'writer')).id).toBe('writer')
    expect(h.get).toHaveBeenLastCalledWith('agent-templates/show', { id: 'writer' })
  })

  it('save posts the contract’s body', async () => {
    h.post.mockResolvedValueOnce({ ok: true })
    await saveTemplate(primaryScope(), { workspace: '/w', id: 'x', wiki: [], dryRun: true })
    expect(h.post).toHaveBeenCalledWith('agent-templates/save', { workspace: '/w', id: 'x', wiki: [], dryRun: true })
  })

  it('an older server’s plain 404 is "can’t yet"; a coded 404 is a real answer', async () => {
    h.post.mockRejectedValueOnce(new DaemonHttpError('route not found', 404, null))
    await expect(saveTemplate(primaryScope(), { workspace: '/w', id: 'x', wiki: [], dryRun: true })).rejects.toThrow(SAVE_UNSUPPORTED_TEXT)
    h.post.mockRejectedValueOnce(new DaemonHttpError('no workspace /w', 404, 'no_workspace'))
    const e = await saveTemplate(primaryScope(), { workspace: '/w', id: 'x', wiki: [], dryRun: true }).catch((x) => x)
    expect(e).not.toBeInstanceOf(TemplatesUnsupportedError)
    expect(saveErrorView(e).text).toBe(SAVE_ERROR_COPY.no_workspace)
    h.get.mockRejectedValueOnce(new DaemonHttpError('route not found', 404, null))
    await expect(listTemplates(primaryScope())).rejects.toThrow(CREATE_UNSUPPORTED_TEXT)
  })

  it('isMissingRoute: plain 404 and the old method guard only', () => {
    expect(isMissingRoute(new DaemonHttpError('route not found', 404, null))).toBe(true)
    expect(isMissingRoute(new DaemonHttpError('method not allowed for this route', 405, null))).toBe(true)
    expect(isMissingRoute(new DaemonHttpError('x', 404, 'not_found'))).toBe(false)
    expect(isMissingRoute(new Error('network'))).toBe(false)
  })
})

describe('save errors', () => {
  it.each(['bad_id', 'id_reserved', 'id_taken', 'own_workspace_only', 'too_large', 'no_workspace'] as const)('%s has its own words', (code) => {
    const v = saveErrorView(new DaemonHttpError('server words', 400, code))
    expect(v.text).toBe(SAVE_ERROR_COPY[code])
    expect(v.fieldsProblem).toBe(code === 'bad_id' || code === 'id_reserved' || code === 'id_taken')
  })

  it('secret_found names the file and line from the error object or the body, never the message', () => {
    const inErr = saveErrorView(new DaemonHttpError('sk-ant-XYZ in a.md', 422, 'secret_found', { code: 'secret_found', file: 'ROLE.md', line: 12 }))
    expect(inErr.text).toBe(secretFoundText('ROLE.md', 12))
    expect(inErr.text).toContain('ROLE.md, line 12')
    expect(inErr.detail).toBeNull()
    const atTop = saveErrorView(new DaemonHttpError('x', 422, 'secret_found', { code: 'secret_found' }, { file: 'context/a.md', line: '4' }))
    expect(atTop.text).toContain('context/a.md, line 4')
    const neither = saveErrorView(new DaemonHttpError('x', 422, 'secret_found'))
    expect(neither.text).toBe(secretFoundText(null, null))
  })

  it('another 403 is the role; anything else keeps the server’s words', () => {
    expect(saveErrorView(new DaemonHttpError('admin only', 403, 'admin_only')).text).toBe(SAVE_ERROR_COPY.forbidden)
    const v = saveErrorView(new DaemonHttpError('disk full', 500, 'internal'))
    expect(v.detail).toBe('disk full')
  })
})

describe('hire errors', () => {
  it('a folder in use or another template’s folder sends the person back to it', () => {
    expect(hireErrorView(new DaemonHttpError('x', 409, 'folder_exists')).folderProblem).toBe(true)
    expect(hireErrorView(new DaemonHttpError('x', 409, 'template_conflict')).folderProblem).toBe(true)
    expect(hireErrorView(new DaemonHttpError('mkdir: Permission denied', 500, 'internal')).folderProblem).toBe(true)
    expect(hireErrorView(new DaemonHttpError('nope', 400, 'usage')).text).toBe('K2 couldn’t make the agent.')
    expect(hireErrorView(new DaemonHttpError('route not found', 404, null)).text).toBe(CREATE_UNSUPPORTED_TEXT)
  })
})
