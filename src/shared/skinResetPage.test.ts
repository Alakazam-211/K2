// agent-app-users T5.10: the box front door's bundled reset page reads the
// set-password token from the #fragment (where K2's invite and reset links
// put it, so it never reaches a server log or a Referer), falls back to an
// older ?token= link, drops the fragment from the address bar, and posts
// the token in the request body.
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

const html = readFileSync(resolve(__dirname, '../../crates/k2-daemon/src/skin_gateway/reset.html'), 'utf8')
const script = (() => {
  const m = html.match(/<script>([\s\S]*?)<\/script>/)
  if (!m) throw new Error('reset.html has no inline script')
  return m[1]
})()

type Posted = { url: string; body: { token: string; password: string } }

async function run(hash: string, search: string): Promise<{ posted: Posted[]; replaced: string[] }> {
  const posted: Posted[] = []
  const replaced: string[] = []
  let submit: ((e: { preventDefault(): void; target: unknown }) => Promise<void>) | null = null
  const el = () => ({ hidden: true, textContent: '' })
  const nodes: Record<string, unknown> = {
    err: el(),
    ok: el(),
    f: {
      addEventListener: (_: string, fn: typeof submit) => {
        submit = fn
      },
    },
  }
  const document = { getElementById: (id: string) => nodes[id] }
  const location = { hash, search, pathname: '/reset' }
  const history = { replaceState: (_s: unknown, _t: string, url: string) => replaced.push(url) }
  const fetch = async (url: string, init: { body: string }) => {
    posted.push({ url, body: JSON.parse(init.body) })
    return { ok: true, status: 200, text: async () => '{"ok":true}' }
  }
  class FormData {
    get(k: string) {
      return k === 'password' ? 'new-password-1' : null
    }
  }
  new Function('document', 'location', 'history', 'fetch', 'FormData', 'URLSearchParams', script)(
    document,
    location,
    history,
    fetch,
    FormData,
    URLSearchParams,
  )
  if (!submit) throw new Error('the page never wired its form')
  await (submit as (e: { preventDefault(): void; target: unknown }) => Promise<void>)({ preventDefault() {}, target: {} })
  return { posted, replaced }
}

describe('reset.html token', () => {
  it('reads #k2inv_… from the fragment and posts it in the body', async () => {
    const { posted, replaced } = await run('#k2inv_abc123', '')
    expect(posted).toEqual([{ url: '/cli/skin/password/reset', body: { token: 'k2inv_abc123', password: 'new-password-1' } }])
    expect(replaced).toEqual(['/reset'])
  })

  it('takes #token=… too', async () => {
    const { posted } = await run('#token=k2inv_def', '')
    expect(posted[0].body.token).toBe('k2inv_def')
  })

  it('falls back to an older ?token= link', async () => {
    const { posted, replaced } = await run('', '?token=legacyhex')
    expect(posted[0].body.token).toBe('legacyhex')
    expect(replaced).toEqual([])
  })

  it('prefers the fragment over the query', async () => {
    const { posted } = await run('#k2inv_frag', '?token=legacyhex')
    expect(posted[0].body.token).toBe('k2inv_frag')
  })
})
