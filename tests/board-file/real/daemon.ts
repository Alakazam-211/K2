// A sandbox k2-daemon for the real-file specs: temp HOME, no client, no Garden, as tests/cli/board_file_headless.sh
// starts one. Never the running daemon, never the real ~/.k2.
import { spawn, type ChildProcess } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync, chmodSync } from 'node:fs'
import { join } from 'node:path'

export interface Sandbox {
  base: string
  token: string
  home: string
  work: string
  start(): Promise<void>
  stop(): Promise<void>
  get(route: string, params?: Record<string, string | number>): Promise<{ status: number; body: any }>
  post(route: string, body: unknown): Promise<{ status: number; body: any }>
  dispose(): void
}

export function daemonBin(): string {
  const bin = process.env.K2_BOARD_DAEMON_BIN
  if (!bin) throw new Error('K2_BOARD_DAEMON_BIN is required: a k2-daemon built from this commit')
  if (!existsSync(bin)) throw new Error(`no k2-daemon at ${bin}`)
  return bin
}

export function createSandbox(): Sandbox {
  const bin = daemonBin()
  // Short root: the cell socket must fit SUN_LEN.
  const work = realpathSync(mkdtempSync('/tmp/k2bft.'))
  const home = join(work, 'h')
  for (const d of ['.k2', 'shim', 'tmp', 'run']) mkdirSync(join(home, d), { recursive: true })
  writeFileSync(join(home, 'shim', 'claude'), '#!/bin/sh\nexec /bin/cat > /dev/null\n')
  chmodSync(join(home, 'shim', 'claude'), 0o755)
  let child: ChildProcess | null = null
  const sb: Sandbox = {
    base: '',
    token: '',
    home,
    work,
    async start() {
      const env: Record<string, string> = {}
      for (const [k, v] of Object.entries(process.env)) if (v !== undefined && !/^(K2|K2SO)_/.test(k)) env[k] = v
      Object.assign(env, {
        HOME: home,
        TMPDIR: join(home, 'tmp'),
        XDG_RUNTIME_DIR: join(home, 'run'),
        K2_RUNTIME_DIR: '',
        K2_TEST_AGENT_SHIM_DIR: join(home, 'shim'),
        K2SO_WATCHDOG_DISABLED: '1',
        K2_HEARTBEAT_NO_SELF_HEAL: '1',
        K2_SUBSCRIPTION_PROBE: 'deny',
      })
      rmSync(join(home, '.k2', 'daemon.port'), { force: true })
      child = spawn(bin, [], { env, stdio: ['ignore', 'ignore', 'ignore'] })
      for (let i = 0; i < 600; i++) {
        const pf = join(home, '.k2', 'daemon.port')
        const tf = join(home, '.k2', 'daemon.token')
        if (existsSync(pf) && existsSync(tf) && readFileSync(pf, 'utf8').trim() && readFileSync(tf, 'utf8').trim()) {
          sb.base = `http://127.0.0.1:${readFileSync(pf, 'utf8').trim()}`
          sb.token = readFileSync(tf, 'utf8').trim()
          try {
            const r = await fetch(`${sb.base}/boot-status`)
            if ((await r.json()).phase === 'ready') return
          } catch {
            // not listening yet
          }
        }
        await new Promise((r) => setTimeout(r, 100))
      }
      throw new Error('the sandbox daemon never reached ready')
    },
    async stop() {
      const c = child
      child = null
      if (!c || c.exitCode !== null) return
      await new Promise<void>((resolve) => {
        c.once('exit', () => resolve())
        c.kill('SIGTERM')
        setTimeout(() => c.kill('SIGKILL'), 10_000)
      })
    },
    async get(route, params = {}) {
      const q = new URLSearchParams({ ...Object.fromEntries(Object.entries(params).map(([k, v]) => [k, String(v)])), token: sb.token })
      const r = await fetch(`${sb.base}/cli/${route}?${q}`)
      return { status: r.status, body: await r.json() }
    },
    async post(route, body) {
      const r = await fetch(`${sb.base}/cli/${route}?token=${encodeURIComponent(sb.token)}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) })
      return { status: r.status, body: await r.json() }
    },
    dispose() {
      if (child) child.kill('SIGKILL')
      rmSync(work, { recursive: true, force: true })
    },
  }
  return sb
}
