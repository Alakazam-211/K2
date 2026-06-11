#!/usr/bin/env bun
// Capture raw PTY bytes from the daemon + log every escape sequence
// we see so we can diagnose which cursor-positioning flavor Claude
// is actually using. Run this, spawn `claude`, type a few chars,
// Cmd+C to stop.
import { readFileSync } from 'node:fs'
import { homedir } from 'node:os'
import { join } from 'node:path'
import { KesselClient } from '../src/renderer/kessel/client'
import type { Frame } from '../src/renderer/kessel/types'

const k2soDir = join(homedir(), '.k2so')
const port = parseInt(
  readFileSync(join(k2soDir, 'heartbeat.port'), 'utf-8').trim(),
  10,
)
const token = readFileSync(join(k2soDir, 'heartbeat.token'), 'utf-8').trim()

const res = await fetch(
  `http://127.0.0.1:${port}/cli/sessions/spawn?token=${token}`,
  {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      agent_name: `capture-${Date.now()}`,
      cwd: process.cwd(),
      command: process.argv[2] ?? 'claude',
      cols: 120,
      rows: 38,
    }),
  },
)
if (!res.ok) {
  console.error('spawn failed:', res.status, await res.text())
  process.exit(1)
}
const spawn = await res.json()
console.log('sessionId:', spawn.sessionId)
console.log('listening for frames... Ctrl-C to stop\n')

// Tally interesting frame types so we can see what Claude emits.
const counts: Record<string, number> = {}
const client = new KesselClient({ sessionId: spawn.sessionId, port, token })
client.on({
  onFrame: (f: Frame) => {
    if (f.frame === 'CursorOp') {
      const op = f.data.op
      counts[op] = (counts[op] || 0) + 1
    } else {
      counts[f.frame] = (counts[f.frame] || 0) + 1
    }
  },
})
client.connect()

// Dump counts every 2 seconds.
setInterval(() => {
  const entries = Object.entries(counts).sort((a, b) => b[1] - a[1])
  console.log(
    '[summary]',
    entries.map(([k, v]) => `${k}=${v}`).join(', ') || '(nothing yet)',
  )
}, 2000)

// Keep alive until Ctrl-C.
process.on('SIGINT', () => {
  console.log('\nfinal counts:', counts)
  client.dispose()
  process.exit(0)
})
