#!/usr/bin/env bun
import { readFileSync } from 'node:fs'
import { homedir } from 'node:os'
import { join } from 'node:path'
import { KesselClient } from '../src/renderer/kessel/client'

const k2soDir = join(homedir(), '.k2so')
const port = parseInt(readFileSync(join(k2soDir, 'heartbeat.port'), 'utf-8').trim(), 10)
const token = readFileSync(join(k2soDir, 'heartbeat.token'), 'utf-8').trim()

const res = await fetch(`http://127.0.0.1:${port}/cli/sessions/spawn?token=${token}`, {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({
    agent_name: `raw-${Date.now()}`,
    cwd: process.cwd(),
    command: 'bash -lc "echo one; sleep 0.2; echo two"',
    cols: 40, rows: 10,
  }),
})
const spawn = await res.json()
const client = new KesselClient({ sessionId: spawn.sessionId, port, token })
const dec = new TextDecoder()
client.on({
  onFrame: (f) => {
    if (f.frame === 'Text') {
      console.log(`[text] bytes=${JSON.stringify(f.data.bytes)} str=${JSON.stringify(dec.decode(new Uint8Array(f.data.bytes)))}`)
    } else if (f.frame === 'CursorOp') {
      console.log(`[cursor] ${JSON.stringify(f.data)}`)
    } else {
      console.log(`[${f.frame}]`)
    }
  },
})
client.connect()
await new Promise(r => setTimeout(r, 3000))
client.dispose()
process.exit(0)
