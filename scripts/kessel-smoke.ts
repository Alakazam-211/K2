#!/usr/bin/env bun
// Kessel smoke harness — validates I2 + I3 + I4 against a live
// daemon without mounting a React app. Spawns a Session Stream
// session via the daemon HTTP API, opens a KesselClient against
// the returned SessionId, feeds incoming Frames into a
// TerminalGrid, and dumps a grid snapshot periodically.
//
// Usage:
//   bun run scripts/kessel-smoke.ts               # default: bash loop
//   bun run scripts/kessel-smoke.ts --cmd "claude --help"
//   bun run scripts/kessel-smoke.ts --cmd "echo hi; sleep 1; echo bye"
//   bun run scripts/kessel-smoke.ts --cmd "..." --duration 10
//
// Requires the daemon to be running (heartbeat.{port,token} present).

import { readFileSync } from 'node:fs'
import { homedir } from 'node:os'
import { join } from 'node:path'

import { KesselClient } from '../src/renderer/kessel/client'
import { TerminalGrid } from '../src/renderer/kessel/grid'

// ── CLI args ──────────────────────────────────────────────────────

const args = process.argv.slice(2)
let command = "for i in 1 2 3 4 5; do printf 'hello-%s\\n' $i; sleep 0.3; done"
let cols = 80
let rows = 24
let durationSecs = 5
for (let i = 0; i < args.length; i++) {
  switch (args[i]) {
    case '--cmd':
      command = args[++i]
      break
    case '--cols':
      cols = parseInt(args[++i], 10)
      break
    case '--rows':
      rows = parseInt(args[++i], 10)
      break
    case '--duration':
      durationSecs = parseInt(args[++i], 10)
      break
    case '-h':
    case '--help':
      console.log(`Usage:
  bun run scripts/kessel-smoke.ts [--cmd <shell-cmd>] [--cols N] [--rows N] [--duration N]

Defaults:
  cmd      = bash loop that echoes hello-1..5 over 1.5s
  cols/rows = 80 / 24
  duration = 5 (seconds to observe frames before exiting)
`)
      process.exit(0)
      break
  }
}

// ── Read daemon addr ───────────────────────────────────────────────

const k2soDir = join(homedir(), '.k2so')
let port: number
let token: string
try {
  port = parseInt(
    readFileSync(join(k2soDir, 'heartbeat.port'), 'utf-8').trim(),
    10,
  )
  token = readFileSync(join(k2soDir, 'heartbeat.token'), 'utf-8').trim()
} catch (e) {
  console.error(`Failed to read ~/.k2so/heartbeat.{port,token}: ${e}`)
  console.error('Is the daemon running?')
  process.exit(1)
}
if (!port || !token) {
  console.error('heartbeat.port/token are empty — daemon misconfigured')
  process.exit(1)
}
console.log(`daemon: 127.0.0.1:${port}`)

// ── Spawn a session via the daemon ─────────────────────────────────

const spawnRes = await fetch(
  `http://127.0.0.1:${port}/cli/sessions/spawn?token=${token}`,
  {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      agent_name: `smoke-${Date.now()}`,
      cwd: process.cwd(),
      command,
      cols,
      rows,
    }),
  },
)
if (!spawnRes.ok) {
  console.error(`spawn failed: HTTP ${spawnRes.status} ${await spawnRes.text()}`)
  process.exit(1)
}
const spawn = await spawnRes.json()
const sessionId = spawn.sessionId as string
console.log(`spawned session: ${sessionId} (agent ${spawn.agentName})`)
console.log(`command: ${command}`)
console.log(`grid:    ${cols} × ${rows}`)
console.log()

// ── Subscribe via KesselClient ─────────────────────────────────────

const grid = new TerminalGrid({ cols, rows })
let frameCount = 0
let textFrameCount = 0
let cursorOpCount = 0
let totalTextBytes = 0

const client = new KesselClient({ sessionId, port, token })
client.on({
  onOpen: () => console.log('[ws] open'),
  onAck: (ack) =>
    console.log(`[ack] sessionId=${ack.sessionId} replayCount=${ack.replayCount}`),
  onFrame: (frame) => {
    frameCount++
    if (frame.frame === 'Text') {
      textFrameCount++
      totalTextBytes += frame.data.bytes.length
    } else if (frame.frame === 'CursorOp') {
      cursorOpCount++
    }
    grid.applyFrame(frame)
  },
  onError: (e) => console.error(`[error] ${e.message}`),
  onClose: (code) => console.log(`[close] code=${code}`),
})
client.connect()

// ── Observe for N seconds, then dump grid ──────────────────────────

await new Promise((r) => setTimeout(r, durationSecs * 1000))

console.log()
console.log(
  `observed: ${frameCount} frames (${textFrameCount} text, ${cursorOpCount} cursor), ${totalTextBytes} text bytes`,
)
console.log()
console.log('── grid snapshot ──')
const snap = grid.snapshot()
for (let r = 0; r < snap.rows; r++) {
  const line = snap.grid[r].map((c) => c.char || ' ').join('')
  console.log(`${String(r).padStart(2)} │${line.replace(/\s+$/, '')}`)
}
if (snap.scrollback.length > 0) {
  console.log()
  console.log(`── scrollback (${snap.scrollback.length} rows) ──`)
  // Just the last 5 for brevity.
  const start = Math.max(0, snap.scrollback.length - 5)
  for (let r = start; r < snap.scrollback.length; r++) {
    const line = snap.scrollback[r].map((c) => c.char || ' ').join('')
    console.log(`${String(r).padStart(2)} │${line.replace(/\s+$/, '')}`)
  }
}
console.log()
console.log(
  `cursor: row=${snap.cursor.row} col=${snap.cursor.col} visible=${snap.cursor.visible}`,
)

client.dispose()
process.exit(0)
