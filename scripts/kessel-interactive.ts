#!/usr/bin/env bun
// Interactive smoke — spawns a bash session via daemon, writes a
// command via /cli/terminal/write (the same endpoint SessionStreamView
// uses), observes the frames that come back. Proves I6's wiring.
import { readFileSync } from 'node:fs'
import { homedir } from 'node:os'
import { join } from 'node:path'
import { KesselClient } from '../src/renderer/kessel/client'
import { TerminalGrid } from '../src/renderer/kessel/grid'

const k2soDir = join(homedir(), '.k2so')
const port = parseInt(
  readFileSync(join(k2soDir, 'heartbeat.port'), 'utf-8').trim(),
  10,
)
const token = readFileSync(join(k2soDir, 'heartbeat.token'), 'utf-8').trim()

async function writeToSession(sessionId: string, text: string, submit: boolean) {
  const params = new URLSearchParams({
    id: sessionId,
    message: text,
    token,
    no_submit: submit ? 'false' : 'true',
  })
  await fetch(`http://127.0.0.1:${port}/cli/terminal/write?${params}`)
}

const res = await fetch(`http://127.0.0.1:${port}/cli/sessions/spawn?token=${token}`, {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({
    agent_name: `interactive-${Date.now()}`,
    cwd: process.cwd(),
    command: 'bash -lc "PS1=\'$ \' bash --noprofile --norc"',
    cols: 80,
    rows: 10,
  }),
})
const spawn = await res.json()
const sessionId = spawn.sessionId
console.log(`spawned ${sessionId}`)

const grid = new TerminalGrid({ cols: 80, rows: 10 })
const client = new KesselClient({ sessionId, port, token })
client.on({
  onFrame: (f) => grid.applyFrame(f),
})
client.connect()

// Wait for the session to be ready + initial prompt.
await new Promise((r) => setTimeout(r, 800))

// Write some commands the way SessionStreamView will — one byte
// stream per key plus an explicit Enter.
await writeToSession(sessionId, 'echo kessel-works', false)
await writeToSession(sessionId, '\r', false)
await new Promise((r) => setTimeout(r, 500))

await writeToSession(sessionId, 'printf "row1\\nrow2\\n"', false)
await writeToSession(sessionId, '\r', false)
await new Promise((r) => setTimeout(r, 500))

console.log('── grid after writes ──')
const snap = grid.snapshot()
for (let r = 0; r < snap.rows; r++) {
  const line = snap.grid[r].map((c) => c.char || ' ').join('')
  console.log(`${String(r).padStart(2)} │${line.replace(/\s+$/, '')}`)
}
console.log(
  `cursor: row=${snap.cursor.row} col=${snap.cursor.col} visible=${snap.cursor.visible}`,
)
client.dispose()
process.exit(0)
