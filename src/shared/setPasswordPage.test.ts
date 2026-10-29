// @vitest-environment jsdom
// K2's own set-password page (crates/k2-daemon/src/skin_gateway/
// set_password.html, 0.46.6): the link's token leaves the address bar at
// once, but the tab keeps it, so a reload (or the same URL pasted again in
// that tab) still sets the password. Set once, the tab forgets it.
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { afterEach, describe, expect, it, vi } from 'vitest'

const HTML = readFileSync(resolve(__dirname, '../../crates/k2-daemon/src/skin_gateway/set_password.html'), 'utf8')
const SCRIPT = (/<script>([\s\S]*?)<\/script>/.exec(HTML) ?? [])[1] ?? ''
const BODY = (/<body>([\s\S]*?)<script>/.exec(HTML) ?? [])[1] ?? ''

/** Load the page as the browser would at `url` (a fresh run of its script). */
function load(url: string): void {
  window.history.replaceState(null, '', url)
  document.body.innerHTML = BODY
  new Function(SCRIPT)()
}

async function submit(password: string): Promise<void> {
  const input = document.querySelector('input[name="password"]') as HTMLInputElement
  input.value = password
  const form = document.getElementById('f') as HTMLFormElement
  form.dispatchEvent(new Event('submit', { cancelable: true }))
  for (let i = 0; i < 5; i++) await Promise.resolve()
}

afterEach(() => {
  sessionStorage.clear()
  vi.unstubAllGlobals()
})

describe('set_password.html', () => {
  it('has a script and a form to test', () => {
    expect(SCRIPT).toContain('/cli/skin/password/reset')
    expect(BODY).toContain('id="f"')
  })

  it('takes the token out of the address bar, keeps it for the tab, and a reload still works', async () => {
    const fetchMock = vi.fn(async () => ({ ok: true, status: 200 }))
    vi.stubGlobal('fetch', fetchMock)
    load('/cli/skin/password/set#k2inv_tok123')
    expect(window.location.hash).toBe('')
    expect(window.location.pathname).toBe('/cli/skin/password/set')
    // A reload (or the bare URL pasted again in this tab): no fragment.
    load('/cli/skin/password/set')
    await submit('a-good-new-password')
    expect(fetchMock).toHaveBeenCalledTimes(1)
    const init = (fetchMock.mock.calls[0] as unknown as [string, { body: string }])[1]
    expect(JSON.parse(init.body)).toEqual({ token: 'k2inv_tok123', password: 'a-good-new-password' })
    expect((document.getElementById('ok') as HTMLElement).hidden).toBe(false)
    // Set: the tab forgets it.
    expect(sessionStorage.getItem('k2-set-password-token')).toBeNull()
  })

  it('a fresh link wins over one the tab kept', async () => {
    const fetchMock = vi.fn(async () => ({ ok: true, status: 200 }))
    vi.stubGlobal('fetch', fetchMock)
    load('/cli/skin/password/set#k2inv_old')
    load('/cli/skin/password/set#token=k2inv_new')
    await submit('a-good-new-password')
    const init = (fetchMock.mock.calls[0] as unknown as [string, { body: string }])[1]
    expect(JSON.parse(init.body).token).toBe('k2inv_new')
  })

  it('with no link and nothing kept it says so and sends nothing', async () => {
    const fetchMock = vi.fn()
    vi.stubGlobal('fetch', fetchMock)
    load('/cli/skin/password/set')
    await submit('a-good-new-password')
    expect(fetchMock).not.toHaveBeenCalled()
    expect((document.getElementById('err') as HTMLElement).textContent).toBe('This page needs the link from your email.')
  })
})
