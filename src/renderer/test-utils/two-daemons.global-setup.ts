// Home M2 — client half of the two-daemon harness (MS51 / MS74).
//
// Spawns this checkout's own `target/debug/k2-daemon` twice, each under a
// fresh temp HOME (the daemon's singleton lock is per HOME), with an empty
// agent shim dir and the watchdog off, exactly like
// `crates/k2-daemon/tests/daemon_port_stability_integration.rs`. Waits for
// `daemon.port`, `daemon.token` and `/boot-status` phase ready, seeds the
// Connect user `anna` (Member) on B with the owner token, and hands the
// ports and owner tokens to the tests through `process.env` (workers start
// after this runs). Never the production daemon, never main's `target/`.

import { spawn, type ChildProcess } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

const REPO = resolve(__dirname, '../../..')
const BINARY = join(REPO, 'target', 'debug', 'k2-daemon')

export const B_USER = 'anna'
export const B_PASSWORD = 'correct horse ms'

export interface Spawned {
  child: ChildProcess
  home: string
  port: number
  owner: string
}

function readTrimmed(path: string): string | null {
  if (!existsSync(path)) return null
  const t = readFileSync(path, 'utf8').trim()
  return t.length > 0 ? t : null
}

async function sleep(ms: number): Promise<void> {
  await new Promise((r) => setTimeout(r, ms))
}

/** Spawn one daemon under a fresh temp HOME. `extraEnv` is for test hooks,
 *  e.g. `K2_TEST_SIMULATE_NO_ATTACH_ONLY=1` for a daemon that behaves like a
 *  released one up to 0.41.6 (Home M5 old-B test). Also used by tests that
 *  need a third daemon; they kill it and remove its HOME themselves. */
export async function spawnDaemon(tag: string, extraEnv: Record<string, string> = {}): Promise<Spawned> {
  const home = mkdtempSync(join(tmpdir(), `k2-ms-${tag}-`))
  mkdirSync(join(home, '.k2'), { recursive: true })
  const shim = join(home, 'agent-shim-empty')
  mkdirSync(shim, { recursive: true })
  // Home M5: a terminal tab in a room runs a shell. `sh` is the one program
  // the shim dir offers (the OS shell, never an agent CLI).
  symlinkSync('/bin/sh', join(shim, 'sh'))
  const child = spawn(BINARY, [], {
    env: { ...process.env, HOME: home, K2_TEST_AGENT_SHIM_DIR: shim, K2SO_WATCHDOG_DISABLED: '1', ...extraEnv },
    stdio: 'ignore',
  })
  const deadline = Date.now() + 20_000
  let port: number | null = null
  let owner: string | null = null
  while (Date.now() < deadline) {
    const p = readTrimmed(join(home, '.k2', 'daemon.port'))
    owner = readTrimmed(join(home, '.k2', 'daemon.token'))
    port = p !== null ? Number(p) : null
    if (port && owner) break
    await sleep(50)
  }
  if (!port || !owner) {
    child.kill('SIGKILL')
    throw new Error(`daemon ${tag} never published daemon.port + daemon.token`)
  }
  const readyBy = Date.now() + 30_000
  for (;;) {
    try {
      const res = await fetch(`http://127.0.0.1:${port}/boot-status`)
      const body = (await res.json()) as { phase?: string }
      if (body.phase === 'ready') break
    } catch {
      // not listening yet
    }
    if (Date.now() > readyBy) {
      child.kill('SIGKILL')
      throw new Error(`daemon ${tag} never reached phase ready`)
    }
    await sleep(100)
  }
  return { child, home, port, owner }
}

/** Seed the Connect user `anna` (Member) with the owner token. */
export async function seedMember(d: Spawned): Promise<void> {
  const add = await fetch(`http://127.0.0.1:${d.port}/cli/users/add?token=${d.owner}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ username: B_USER, password: B_PASSWORD }),
  })
  if (add.status !== 200) throw new Error(`seeding ${B_USER} on ${d.port} failed: ${add.status} ${await add.text()}`)
}

let spawned: Spawned[] = []

export async function setup(): Promise<void> {
  if (!existsSync(BINARY)) {
    throw new Error(`no daemon binary at ${BINARY}: run \`cargo build -p k2-daemon\` in this checkout first`)
  }
  const a = await spawnDaemon('a')
  spawned.push(a)
  const b = await spawnDaemon('b')
  spawned.push(b)
  await seedMember(b)
  process.env.K2_MS_A_PORT = String(a.port)
  process.env.K2_MS_A_OWNER = a.owner
  process.env.K2_MS_B_PORT = String(b.port)
  process.env.K2_MS_B_OWNER = b.owner
  process.env.K2_MS_B_USER = B_USER
  process.env.K2_MS_B_PASSWORD = B_PASSWORD
}

export async function teardown(): Promise<void> {
  for (const s of spawned) {
    s.child.kill('SIGKILL')
    rmSync(s.home, { recursive: true, force: true })
  }
  spawned = []
}
