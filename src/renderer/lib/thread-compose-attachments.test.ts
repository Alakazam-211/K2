// @vitest-environment jsdom
// S4: a person's files on a Thread post (`thread-attachments-v1`). Fail loud.
//
//   1. A local file inside the Thread's workspace is attached where it is;
//      one outside it is uploaded into `.k2/downloads` and that path is
//      attached; on another server every file is uploaded; browser Files
//      are uploaded. A cancelled/failed upload attaches nothing.
//   2. postThreadCompose sends `attachments` in the JSON body (text may be
//      empty) and turns an `attachment_*` refusal into words.
//   3. Merge: no path twice, at most 10.
import { beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  executeRemoteDrop: vi.fn(),
  executeBrowserFileDrop: vi.fn(),
  daemonCliPost: vi.fn(),
}))

vi.mock('@/lib/handle-remote-drop', () => ({
  executeRemoteDrop: h.executeRemoteDrop,
  executeBrowserFileDrop: h.executeBrowserFileDrop,
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliPost: h.daemonCliPost,
  daemonCliGet: vi.fn(),
}))

import type { ServerScope } from '@/kessel/server-scope'
import { fakeScope } from '@/test-utils/fake-scope'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { postThreadCompose } from '@/components/SessionView/overlayThread'
import {
  isInsideWorkspace,
  mergeThreadComposeFiles,
  readThreadComposeFiles,
  resolveThreadComposeFiles,
  threadAttachmentRefusalText,
  threadComposeAttachOn,
  threadComposeFile,
  writeThreadComposeFiles,
  THREAD_POST_MAX_FILES,
} from './thread-compose-attachments'

const WS = '/Users/me/ai/sales'
const LOCAL = fakeScope('local', { remote: false })
const REMOTE = fakeScope('box.example', { remote: true })

/** The upload stand-in: each file lands in `.k2/downloads` and the
 *  builder sees the server paths, as `runRemoteDropUploads` does. */
function uploadsTo(dir: string) {
  return async (_s: ServerScope, local: string[], _t: unknown, _c: unknown, build?: (r: string[]) => string) => {
    const remote = local.map((p) => `${dir}/${p.split('/').pop()}`)
    return build ? build(remote) : null
  }
}

beforeEach(() => {
  h.executeRemoteDrop.mockReset()
  h.executeBrowserFileDrop.mockReset()
  h.daemonCliPost.mockReset()
  localStorage.clear()
})

describe('resolveThreadComposeFiles', () => {
  it('this computer: a file inside the workspace is attached where it is, no upload', async () => {
    const got = await resolveThreadComposeFiles(LOCAL, { paths: [`${WS}/out/hero.png`], workspacePath: WS })
    expect(got).toEqual([{ path: `${WS}/out/hero.png`, name: 'hero.png' }])
    expect(h.executeRemoteDrop).not.toHaveBeenCalled()
  })

  it('this computer: a file outside the workspace is uploaded into .k2/downloads, then attached (order kept)', async () => {
    h.executeRemoteDrop.mockImplementation(uploadsTo(`${WS}/.k2/downloads`))
    const got = await resolveThreadComposeFiles(LOCAL, {
      paths: ['/Users/me/Desktop/notes.pdf', `${WS}/src/a.rs`, '/tmp/shot.png'],
      workspacePath: WS,
    })
    expect(h.executeRemoteDrop).toHaveBeenCalledTimes(1)
    const [scope, local, target, ctx] = h.executeRemoteDrop.mock.calls[0]
    expect(scope).toBe(LOCAL)
    expect(local).toEqual(['/Users/me/Desktop/notes.pdf', '/tmp/shot.png'])
    expect(target).toEqual({ kind: 'terminal' })
    expect(ctx).toEqual({ workspacePath: WS })
    expect(got?.map((f) => f.path)).toEqual([
      `${WS}/.k2/downloads/notes.pdf`,
      `${WS}/src/a.rs`,
      `${WS}/.k2/downloads/shot.png`,
    ])
  })

  it('another server: every local file is uploaded (even one at a path that looks inside)', async () => {
    h.executeRemoteDrop.mockImplementation(uploadsTo('/home/k2/ai/sales/.k2/downloads'))
    const got = await resolveThreadComposeFiles(REMOTE, { paths: [`${WS}/out/hero.png`], workspacePath: '/home/k2/ai/sales' })
    expect(h.executeRemoteDrop.mock.calls[0][1]).toEqual([`${WS}/out/hero.png`])
    expect(got).toEqual([{ path: '/home/k2/ai/sales/.k2/downloads/hero.png', name: 'hero.png' }])
  })

  it('browser Files are uploaded and their server paths attached', async () => {
    h.executeBrowserFileDrop.mockImplementation(
      async (_s: unknown, files: File[], _t: unknown, _c: unknown, build: (r: string[]) => string) =>
        build(files.map((f) => `${WS}/.k2/downloads/${f.name}`)),
    )
    const file = new File(['x'], 'drop.txt', { type: 'text/plain' })
    const got = await resolveThreadComposeFiles(REMOTE, { files: [file], workspacePath: WS })
    expect(h.executeBrowserFileDrop.mock.calls[0][1]).toEqual([file])
    expect(got).toEqual([{ path: `${WS}/.k2/downloads/drop.txt`, name: 'drop.txt' }])
  })

  it('a cancelled or failed upload attaches nothing', async () => {
    h.executeRemoteDrop.mockResolvedValue(null)
    expect(await resolveThreadComposeFiles(REMOTE, { paths: ['/a/b.png'], workspacePath: WS })).toBeNull()
    h.executeBrowserFileDrop.mockResolvedValue(null)
    expect(await resolveThreadComposeFiles(REMOTE, { files: [new File(['x'], 'b')], workspacePath: WS })).toBeNull()
  })

  it('an upload that joined another one already running says so (no wrong paths)', async () => {
    h.executeRemoteDrop.mockResolvedValue('/elsewhere/b.png ')
    await expect(resolveThreadComposeFiles(REMOTE, { paths: ['/a/b.png'], workspacePath: WS })).rejects.toThrow(
      /already uploading/,
    )
  })
})

describe('helpers', () => {
  it('isInsideWorkspace: under the root only, never the root itself or a sibling', () => {
    expect(isInsideWorkspace(`${WS}/a.md`, WS)).toBe(true)
    expect(isInsideWorkspace(`${WS}/a.md`, `${WS}/`)).toBe(true)
    expect(isInsideWorkspace(WS, WS)).toBe(false)
    expect(isInsideWorkspace(`${WS}-old/a.md`, WS)).toBe(false)
    expect(isInsideWorkspace('/a.md', undefined)).toBe(false)
  })

  it('merge: no path twice, at most 10, the rest counted', () => {
    const cur = [threadComposeFile('/w/a')]
    const many = Array.from({ length: 12 }, (_, i) => threadComposeFile(`/w/f${i}`))
    const out = mergeThreadComposeFiles(cur, [threadComposeFile('/w/a'), ...many])
    expect(out.files).toHaveLength(THREAD_POST_MAX_FILES)
    expect(out.files[0].path).toBe('/w/a')
    expect(out.dropped).toBe(3)
  })

  it('the chips survive a restart beside the draft', () => {
    writeThreadComposeFiles('k', [threadComposeFile('/w/a.png')])
    expect(readThreadComposeFiles('k')).toEqual([{ path: '/w/a.png', name: 'a.png' }])
    writeThreadComposeFiles('k', [])
    expect(localStorage.getItem('k')).toBeNull()
  })

  it('the gate is the server\'s thread-attachments-v1', () => {
    expect(threadComposeAttachOn({ serverSupports: (f: string) => f === 'thread-attachments-v1' })).toBe(true)
    expect(threadComposeAttachOn({ serverSupports: () => false })).toBe(false)
  })

  it('refusal words: the daemon hint first, ours for a bare code, null for anything else', () => {
    const hinted = new DaemonHttpError('copy it into the workspace (e.g. out/)', 400, 'attachment_outside_workspace')
    expect(threadAttachmentRefusalText(hinted, hinted.message)).toBe('copy it into the workspace (e.g. out/)')
    const bare = new DaemonHttpError('{"error":{"code":"attachment_secret"}}', 400, 'attachment_secret')
    expect(threadAttachmentRefusalText(bare, bare.message)).toMatch(/vault/)
    expect(threadAttachmentRefusalText(new DaemonHttpError('nope', 403, 'forbidden'), 'nope')).toBeNull()
  })
})

describe('postThreadCompose with files', () => {
  it('sends attachments in the JSON body; text may be empty', async () => {
    h.daemonCliPost.mockResolvedValue({ ok: true, id: 'd1', seq: 3, kind: 'text', from: 'owner', body: '', attachments: [] })
    const r = await postThreadCompose(LOCAL, 'sales', '', null, {
      attachments: [{ path: `${WS}/out/hero.png` }, { path: `${WS}/src/a.rs`, line: 12, lineEnd: 40 }],
    })
    expect(r.ok).toBe(true)
    expect(h.daemonCliPost).toHaveBeenCalledWith(LOCAL, 'thread/post', {
      addr: 'sales',
      text: '',
      via: 'compose',
      attachments: [{ path: `${WS}/out/hero.png` }, { path: `${WS}/src/a.rs`, line: 12, lineEnd: 40 }],
    })
  })

  it('no files: the body has no attachments key (an older server sees today\'s post)', async () => {
    h.daemonCliPost.mockResolvedValue({ ok: true, id: 'd1', seq: 3 })
    await postThreadCompose(LOCAL, 'sales', 'hi', null, { attachments: [] })
    expect(h.daemonCliPost.mock.calls[0][2]).toEqual({ addr: 'sales', text: 'hi', via: 'compose' })
  })

  it('a refused file: ok false with the daemon\'s hint and code', async () => {
    h.daemonCliPost.mockRejectedValue(
      new DaemonHttpError('secrets travel through the vault (k2 vault), not the Thread', 400, 'attachment_secret'),
    )
    const r = await postThreadCompose(LOCAL, 'sales', 'see', null, { attachments: [{ path: `${WS}/.env` }] })
    expect(r).toEqual({ ok: false, error: 'secrets travel through the vault (k2 vault), not the Thread', code: 'attachment_secret' })
  })
})
