// prd-app-frame-ancestors-v1 §5 test 7 (FA15) + FA19 positive embed.
// See playwright.config.ts for how to build and run.
import { test, expect, type Page } from '@playwright/test'
import { spawn, type ChildProcess } from 'child_process'
import * as fs from 'fs'
import * as http from 'http'
import * as net from 'net'
import * as os from 'os'
import * as path from 'path'

const repoRoot = path.resolve(__dirname, '..', '..')
const BIN =
  process.env.K2_FRAME_TEST_DAEMON_BIN ??
  path.join(process.env.CARGO_TARGET_DIR ?? path.join(repoRoot, 'target'), 'debug', 'k2-daemon')

const APP_DEFAULT = "'self' tauri://localhost http://tauri.localhost"

function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const s = net.createServer()
    s.once('error', reject)
    s.listen(0, '127.0.0.1', () => {
      const addr = s.address()
      if (addr === null || typeof addr === 'string') {
        reject(new Error('no port'))
        return
      }
      s.close(() => resolve(addr.port))
    })
  })
}

function portUp(port: number): Promise<boolean> {
  return new Promise((resolve) => {
    const c = net.connect(port, '127.0.0.1')
    c.once('connect', () => {
      c.destroy()
      resolve(true)
    })
    c.once('error', () => resolve(false))
  })
}

const procs: ChildProcess[] = []
const servers: http.Server[] = []

/// A helper on `port`. `sources` = the `--frame-ancestors` value, or
/// undefined for "no flag" (the helper's own default).
async function startHelper(port: number, sources?: string): Promise<void> {
  if (!fs.existsSync(BIN)) {
    throw new Error(`k2-daemon binary not found at ${BIN}; build it first (cargo build -p k2-daemon --bin k2-daemon)`)
  }
  // Upstream is a closed port: the login page is static, nothing proxies.
  const dead = await freePort()
  const args = ['--skin-gateway', '--listen', `127.0.0.1:${port}`, '--upstream', `http://127.0.0.1:${dead}`]
  if (sources !== undefined) args.push('--frame-ancestors', sources)
  const env: NodeJS.ProcessEnv = {}
  for (const [k, v] of Object.entries(process.env)) {
    if (!/^(K2|K2SO)_/.test(k)) env[k] = v
  }
  env.HOME = fs.mkdtempSync(path.join(os.tmpdir(), 'k2-frame-'))
  env.K2_PUBLISH_HELPER_EXIT_WITH_PARENT = '1'
  const p = spawn(BIN, args, { env, stdio: ['ignore', 'pipe', 'pipe'] })
  let stderr = ''
  p.stderr?.on('data', (d) => (stderr += String(d)))
  procs.push(p)
  const deadline = Date.now() + 15_000
  while (Date.now() < deadline) {
    if (p.exitCode !== null) throw new Error(`helper exited ${p.exitCode}: ${stderr}`)
    if (await portUp(port)) return
    await new Promise((r) => setTimeout(r, 100))
  }
  throw new Error(`helper never listened on ${port}: ${stderr}`)
}

/// The framing page on http://127.0.0.1:<port>/?src=<url>.
async function startFramer(port: number): Promise<void> {
  const server = http.createServer((req, res) => {
    const u = new URL(req.url ?? '/', `http://127.0.0.1:${port}`)
    const src = u.searchParams.get('src') ?? ''
    res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' })
    res.end(`<!doctype html><title>framer</title><iframe id="app" src="${src.replace(/"/g, '&quot;')}" width="600" height="400"></iframe>`)
  })
  servers.push(server)
  await new Promise<void>((resolve) => server.listen(port, '127.0.0.1', resolve))
}

function headOf(url: string): Promise<http.IncomingHttpHeaders> {
  return new Promise((resolve, reject) => {
    const r = http.request(url, { method: 'HEAD' }, (res) => {
      res.resume()
      resolve(res.headers)
    })
    r.once('error', reject)
    r.end()
  })
}

/// True when some frame of the page shows the App's login form.
async function appRendered(page: Page, appOrigin: string): Promise<boolean> {
  for (const f of page.frames()) {
    if (f === page.mainFrame() || !f.url().startsWith(appOrigin)) continue
    const n = await f.locator('input[type=password]').count()
    if (n > 0) return true
  }
  return false
}

async function framePage(page: Page, framerPort: number, appUrl: string): Promise<void> {
  await page.goto(`http://127.0.0.1:${framerPort}/?src=${encodeURIComponent(appUrl)}`)
  // Wait for the frame's navigation to finish (blocked or not).
  await page.waitForLoadState('load')
}

test.afterAll(async () => {
  for (const p of procs) p.kill('SIGTERM')
  for (const s of servers) s.close()
})

test('default policy: another origin cannot render the App in a frame', async ({ page }) => {
  const framer = await freePort()
  const app = await freePort()
  await startFramer(framer)
  await startHelper(app, APP_DEFAULT)
  const appOrigin = `http://localhost:${app}`
  const h = await headOf(`${appOrigin}/login`)
  expect(h['content-security-policy']).toBe(`frame-ancestors ${APP_DEFAULT}`)
  expect(h['x-frame-options']).toBe('SAMEORIGIN')

  await framePage(page, framer, `${appOrigin}/login`)
  // Give a slow engine time to (not) paint the form.
  await page.waitForTimeout(1500)
  expect(await appRendered(page, appOrigin)).toBe(false)

  // Control: the same App renders top-level, so the refusal above is the
  // frame rule and not a dead helper.
  const top = await page.context().newPage()
  await top.goto(`${appOrigin}/login`)
  await expect(top.locator('input[type=password]')).toHaveCount(1)
  await top.close()
})

test("no flag (hand-started helper) is the same default", async ({ page }) => {
  const framer = await freePort()
  const app = await freePort()
  await startFramer(framer)
  await startHelper(app)
  const appOrigin = `http://localhost:${app}`
  const h = await headOf(`${appOrigin}/login`)
  expect(h['content-security-policy']).toBe(`frame-ancestors ${APP_DEFAULT}`)
  await framePage(page, framer, `${appOrigin}/login`)
  await page.waitForTimeout(1500)
  expect(await appRendered(page, appOrigin)).toBe(false)
})

test("'none' refuses every frame", async ({ page }) => {
  const framer = await freePort()
  const app = await freePort()
  await startFramer(framer)
  await startHelper(app, "'none'")
  const appOrigin = `http://localhost:${app}`
  const h = await headOf(`${appOrigin}/login`)
  expect(h['content-security-policy']).toBe("frame-ancestors 'none'")
  expect(h['x-frame-options']).toBe('DENY')
  await framePage(page, framer, `${appOrigin}/login`)
  await page.waitForTimeout(1500)
  expect(await appRendered(page, appOrigin)).toBe(false)
})

test('allowed origin really embeds the App, with X-Frame-Options still sent (FA19)', async ({ page }) => {
  const framer = await freePort()
  const app = await freePort()
  await startFramer(framer)
  // What `k2 publish frame <name> --allow http://127.0.0.1:<framer>` produces.
  await startHelper(app, `${APP_DEFAULT} http://127.0.0.1:${framer}`)
  const appOrigin = `http://localhost:${app}`
  const h = await headOf(`${appOrigin}/login`)
  expect(h['content-security-policy']).toBe(`frame-ancestors ${APP_DEFAULT} http://127.0.0.1:${framer}`)
  // SAMEORIGIN alone would refuse this cross-origin frame; the engine must
  // ignore it because frame-ancestors is present.
  expect(h['x-frame-options']).toBe('SAMEORIGIN')

  await framePage(page, framer, `${appOrigin}/login`)
  await expect.poll(() => appRendered(page, appOrigin), { timeout: 10_000 }).toBe(true)
})
