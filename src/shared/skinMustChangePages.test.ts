// Must-change-password at next sign-in (0.46.6): the gateway's bundled
// login page sends a person whose password someone else set to K2's
// set-password page with the one-time change token in the #fragment, and
// the set-password page says plainly when the new password is the
// temporary one. Both pages run here with stubbed browser globals.
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

const dir = resolve(__dirname, '../../crates/k2-daemon/src/skin_gateway')
function scriptOf(file: string): string {
  const html = readFileSync(resolve(dir, file), 'utf8')
  const m = html.match(/<script>([\s\S]*?)<\/script>/)
  if (!m) throw new Error(`${file} has no inline script`)
  return m[1]
}

type Answer = { status: number; body: string }
type Node = { hidden: boolean; textContent: string }
type Submit = (e: { preventDefault(): void; target: unknown }) => Promise<void>

const TOKEN = 'k2chg_' + 'ab12'.repeat(16)

async function runPage(
  file: string,
  answer: Answer,
  hash = '',
): Promise<{ posted: { url: string; body: Record<string, unknown> }[]; nav: string[]; err: Node; ok: Node }> {
  const posted: { url: string; body: Record<string, unknown> }[] = []
  const nav: string[] = []
  let submit: Submit | null = null
  const err: Node = { hidden: true, textContent: '' }
  const ok: Node = { hidden: true, textContent: '' }
  const nodes: Record<string, unknown> = {
    err,
    ok,
    f: {
      addEventListener: (_: string, fn: Submit) => {
        submit = fn
      },
    },
  }
  const document = { getElementById: (id: string) => nodes[id] }
  const location = {
    hash,
    search: '',
    pathname: file === 'login.html' ? '/login' : '/cli/skin/password/set',
    set href(v: string) {
      nav.push(v)
    },
    get href() {
      return nav[nav.length - 1] ?? ''
    },
  }
  const history = { replaceState: () => {} }
  const store: Record<string, string> = {}
  const sessionStorage = {
    getItem: (k: string) => store[k] ?? null,
    setItem: (k: string, v: string) => {
      store[k] = v
    },
    removeItem: (k: string) => {
      delete store[k]
    },
  }
  const fetch = async (url: string, init: { body: string }) => {
    posted.push({ url, body: JSON.parse(init.body) })
    return {
      ok: answer.status >= 200 && answer.status < 300,
      status: answer.status,
      text: async () => answer.body,
      json: async () => JSON.parse(answer.body),
    }
  }
  class FormData {
    get(k: string) {
      return k === 'password' ? 'typed-password-1' : k === 'username' ? 'dana' : null
    }
  }
  new Function('document', 'location', 'history', 'fetch', 'FormData', 'URLSearchParams', 'sessionStorage', scriptOf(file))(
    document,
    location,
    history,
    fetch,
    FormData,
    URLSearchParams,
    sessionStorage,
  )
  if (!submit) throw new Error(`${file} never wired its form`)
  await (submit as Submit)({ preventDefault() {}, target: {} })
  return { posted, nav, err, ok }
}

const mustChange = (token: string = TOKEN): Answer => ({
  status: 403,
  body: JSON.stringify({
    ok: false,
    error: { code: 'password_change_required', hint: 'choose a new password' },
    mustChangePassword: true,
    changeToken: token,
    changeExpiresAt: 1,
    changePath: '/cli/skin/password/set',
    changeUrl: null,
    username: 'dana',
    app: 'svc-1',
  }),
})

describe('login.html: must change password', () => {
  it('sends the person to K2’s set-password page with the change token in the fragment', async () => {
    const { posted, nav, err } = await runPage('login.html', mustChange())
    expect(posted).toEqual([{ url: '/login', body: { username: 'dana', password: 'typed-password-1' } }])
    expect(nav).toEqual([`/cli/skin/password/set#${TOKEN}`])
    expect(err.hidden).toBe(true)
  })

  it('never follows a token that isn’t a change token', async () => {
    for (const bad of ['k2skn_abc', 'k2inv_abc', 'k2chg_../x', '']) {
      const { nav, err } = await runPage('login.html', mustChange(bad))
      expect(nav).toEqual([])
      expect(err.hidden).toBe(false)
      expect(err.textContent).toBe('invalid username or password')
    }
  })

  it('a wrong password is still the plain error, and a right one goes home', async () => {
    const bad = await runPage('login.html', { status: 401, body: '{"error":"invalid username or password"}' })
    expect(bad.nav).toEqual([])
    expect(bad.err.textContent).toBe('invalid username or password')
    const good = await runPage('login.html', { status: 200, body: '{"ok":true,"username":"dana"}' })
    expect(good.nav).toEqual(['/'])
  })
})

describe('set_password.html: the change token', () => {
  it('posts the change token from the fragment to the reset route', async () => {
    const { posted, ok } = await runPage('set_password.html', { status: 200, body: '{"ok":true}' }, `#${TOKEN}`)
    expect(posted).toEqual([{ url: '/cli/skin/password/reset', body: { token: TOKEN, password: 'typed-password-1' } }])
    expect(ok.hidden).toBe(false)
  })

  it('says so when the new password is the temporary one', async () => {
    const answer = { status: 400, body: '{"error":{"code":"password_unchanged","hint":"choose a new password"}}' }
    const { err, ok } = await runPage('set_password.html', answer, `#${TOKEN}`)
    expect(err.hidden).toBe(false)
    expect(err.textContent).toBe('Choose a new password, not the temporary one you were given.')
    expect(ok.hidden).toBe(true)
  })

  it('keeps the other errors as they were', async () => {
    const short = await runPage('set_password.html', { status: 400, body: '{"error":"password must be at least 8 characters"}' }, `#${TOKEN}`)
    expect(short.err.textContent).toBe('The password must be at least 8 characters.')
    const used = await runPage('set_password.html', { status: 401, body: '{"error":"invalid or expired reset token"}' }, `#${TOKEN}`)
    expect(used.err.textContent).toBe('This link was already used or has expired.')
  })
})
