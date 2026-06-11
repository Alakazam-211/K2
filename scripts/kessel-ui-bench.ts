#!/usr/bin/env bun
/**
 * Kessel UI-layer bench: measures the OUTSIDE-OF-REACT portion of
 * the Kessel pipeline that the frontend can't escape —
 *
 *     keydown → writeToSession → daemon → PTY → daemon WS → browser
 *
 * The React render step happens AFTER the WS message lands, so any
 * gap between an observed latency here and the user-visible total
 * is squarely in the React pipeline (reconciliation, styled spans,
 * damage tracking, etc.).
 *
 * Run (daemon must be alive):
 *   bun run scripts/kessel-ui-bench.ts
 *
 * Exit code is 0 iff all tests pass.
 */

import { readFileSync } from 'node:fs'
import { homedir } from 'node:os'
import { join } from 'node:path'

const k2soDir = join(homedir(), '.k2so')
const port = parseInt(
  readFileSync(join(k2soDir, 'heartbeat.port'), 'utf-8').trim(),
  10,
)
const token = readFileSync(join(k2soDir, 'heartbeat.token'), 'utf-8').trim()

type Frame =
  | { frame: 'Text'; data: { bytes: number[]; style: null } }
  | { frame: 'CursorOp'; data: any }
  | { frame: string; data: any }

type Envelope =
  | { event: 'session:ack'; payload: { sessionId: string; replayCount: number } }
  | { event: 'session:frame'; payload: Frame }
  | { event: 'session:error'; payload: { message: string } }

interface SpawnResult {
  sessionId: string
  agentName: string
}

async function spawnSession(command: string, args: string[]): Promise<SpawnResult> {
  const res = await fetch(
    `http://127.0.0.1:${port}/cli/sessions/spawn?token=${token}`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        agent_name: `uibench-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
        cwd: '/tmp',
        command,
        args,
        cols: 80,
        rows: 24,
      }),
    },
  )
  if (!res.ok) throw new Error(`spawn failed: ${res.status} ${await res.text()}`)
  return (await res.json()) as SpawnResult
}

async function writeToSession(sessionId: string, text: string): Promise<void> {
  const params = new URLSearchParams({
    id: sessionId,
    message: text,
    token,
    no_submit: 'true',
  })
  const url = `http://127.0.0.1:${port}/cli/terminal/write?${params}`
  const res = await fetch(url, { method: 'GET' })
  if (!res.ok) throw new Error(`write failed: ${res.status}`)
}

function openSubscription(sessionId: string) {
  const url = `ws://127.0.0.1:${port}/cli/sessions/subscribe?session=${sessionId}&token=${token}`
  const ws = new WebSocket(url)
  const frames: { env: Envelope; arrivedAt: number }[] = []
  let ackAt: number | null = null
  let openAt: number | null = null
  const openedP = new Promise<void>((resolve, reject) => {
    ws.onopen = () => {
      openAt = performance.now()
      resolve()
    }
    ws.onerror = (e) => reject(e)
  })
  ws.onmessage = (ev) => {
    const env = JSON.parse(String(ev.data)) as Envelope
    frames.push({ env, arrivedAt: performance.now() })
    if (env.event === 'session:ack') {
      ackAt = performance.now()
    }
  }
  return {
    ws,
    frames,
    opened: openedP,
    getAckMs: () => (openAt && ackAt ? ackAt - openAt : null),
    waitFrames: (pred: (f: Frame) => boolean, timeoutMs = 5_000) =>
      new Promise<{ frame: Frame; elapsed: number }>((resolve, reject) => {
        const started = performance.now()
        const check = () => {
          for (let i = frames.length - 1; i >= 0; i--) {
            const f = frames[i].env
            if (f.event === 'session:frame' && pred(f.payload)) {
              resolve({ frame: f.payload, elapsed: frames[i].arrivedAt - started })
              return
            }
          }
          if (performance.now() - started > timeoutMs) {
            reject(new Error('waitFrames: timeout'))
            return
          }
          setTimeout(check, 2)
        }
        check()
      }),
    close: () => ws.close(),
  }
}

function frameIncludesByte(byte: number): (f: Frame) => boolean {
  return (f: Frame) =>
    f.frame === 'Text' && (f.data as any).bytes.some((b: number) => b === byte)
}

function fmtMs(ms: number): string {
  return `${ms.toFixed(1)}ms`
}

function percentile(xs: number[], p: number): number {
  const sorted = [...xs].sort((a, b) => a - b)
  const idx = Math.min(sorted.length - 1, Math.floor((sorted.length - 1) * p))
  return sorted[idx]
}

// ── Test 1 — WS round-trip latency (write → echo) ───────────────────
//
// Spawn an interactive shell. Send a single character. Time from
// the HTTP write to the echo frame arriving via WS. This is the
// pure daemon → PTY → daemon → WS pipeline cost; React is NOT in
// the loop here.

async function testWsRoundTrip(): Promise<void> {
  console.log('\n[Test 1] WS round-trip latency (write → echo)')
  console.log('-'.repeat(72))

  const { sessionId } = await spawnSession('bash', [])
  const sub = openSubscription(sessionId)
  await sub.opened
  // Wait for shell prompt so the PTY is settled before measuring.
  await new Promise((r) => setTimeout(r, 300))

  // Fire 20 single-character writes, measure echo latency per write.
  const results: number[] = []
  for (let i = 0; i < 20; i++) {
    // Use a uniquely-identifiable character per iteration — 'a'..'t'.
    const char = String.fromCharCode(0x61 + i)
    const byte = char.charCodeAt(0)
    const preCount = sub.frames.length
    const startT = performance.now()
    await writeToSession(sessionId, char)
    // Wait for a frame with the echoed byte (after preCount).
    const deadline = performance.now() + 2_000
    let echo: number | null = null
    while (performance.now() < deadline) {
      for (let j = preCount; j < sub.frames.length; j++) {
        const env = sub.frames[j].env
        if (
          env.event === 'session:frame' &&
          env.payload.frame === 'Text' &&
          (env.payload.data as any).bytes.includes(byte)
        ) {
          echo = sub.frames[j].arrivedAt - startT
          break
        }
      }
      if (echo !== null) break
      await new Promise((r) => setTimeout(r, 1))
    }
    if (echo === null) {
      console.log(`  #${i + 1} '${char}': TIMEOUT (>2s)`)
    } else {
      results.push(echo)
      // Only log every 5 for readability
      if (i === 0 || i === 9 || i === 19) {
        console.log(`  #${i + 1} '${char}': ${fmtMs(echo)}`)
      }
    }
    // Tiny gap between writes so PTY settles.
    await new Promise((r) => setTimeout(r, 30))
  }
  sub.close()

  if (results.length > 0) {
    const min = Math.min(...results)
    const max = Math.max(...results)
    const mean = results.reduce((a, b) => a + b, 0) / results.length
    const p50 = percentile(results, 0.5)
    const p95 = percentile(results, 0.95)
    console.log(
      `\n  Summary: n=${results.length}  min=${fmtMs(min)}  p50=${fmtMs(p50)}  mean=${fmtMs(mean)}  p95=${fmtMs(p95)}  max=${fmtMs(max)}`,
    )
    if (p50 < 15) {
      console.log(`  ✅ p50 ${fmtMs(p50)} is inside the "feels instant" threshold.`)
    } else if (p50 < 50) {
      console.log(`  ⚠️  p50 ${fmtMs(p50)} is noticeable — typing will feel slightly behind.`)
    } else {
      console.log(`  ❌ p50 ${fmtMs(p50)} is way too slow — typing will feel laggy.`)
    }
  }
}

// ── Test 2 — Sustained throughput (100 chars batched) ───────────────
//
// Send 100 characters in rapid succession. Measure time from first
// write to last echoed byte. Exposes daemon + PTY throughput under
// burst load — the scenario that corresponds to a user typing fast
// or pasting a block of text.

async function testSustainedThroughput(): Promise<void> {
  console.log('\n[Test 2] Sustained throughput (100 chars, as-fast-as-possible)')
  console.log('-'.repeat(72))

  const { sessionId } = await spawnSession('bash', [])
  const sub = openSubscription(sessionId)
  await sub.opened
  await new Promise((r) => setTimeout(r, 300))

  const chars = 'abcdefghijklmnopqrstuvwxyz0123456789'.repeat(3).slice(0, 100)
  const startAll = performance.now()
  // Fire all writes in parallel (simulates a paste burst).
  await Promise.all(
    [...chars].map((c) => writeToSession(sessionId, c).catch(() => {})),
  )
  const writesSentAt = performance.now()

  // Wait for the 100th echo — or give up after 3s. We look for the
  // last character since order isn't guaranteed, but the PTY echoes
  // in order so the last char SHOULD arrive last.
  const lastByte = chars.charCodeAt(chars.length - 1)
  const deadline = performance.now() + 3_000
  let finishedAt: number | null = null
  while (performance.now() < deadline) {
    const matches = sub.frames.filter((f) => {
      const env = f.env
      return (
        env.event === 'session:frame' &&
        env.payload.frame === 'Text' &&
        (env.payload.data as any).bytes.includes(lastByte)
      )
    })
    if (matches.length > 0) {
      finishedAt = matches[matches.length - 1].arrivedAt
      break
    }
    await new Promise((r) => setTimeout(r, 2))
  }
  sub.close()

  const writesTime = writesSentAt - startAll
  if (finishedAt === null) {
    console.log(`  Writes completed in ${fmtMs(writesTime)}, but last echo timed out.`)
    return
  }
  const total = finishedAt - startAll
  console.log(`  Writes dispatched:          ${fmtMs(writesTime)}`)
  console.log(`  Full round-trip (100 ch):   ${fmtMs(total)}`)
  console.log(`  Per-char amortized:         ${fmtMs(total / chars.length)}`)
  if (total < 500) {
    console.log(`  ✅ Burst keeps up — no visible lag at 100 cps.`)
  } else if (total < 1_500) {
    console.log(`  ⚠️  Burst is slow — typing fast will accumulate lag.`)
  } else {
    console.log(`  ❌ Burst can't keep up — paste lag likely.`)
  }
}

// ── Test 3 — Stale-pane throughput (pre-populated with output) ──────
//
// Spawn a session, run a command that emits ~5KB of output so the
// broadcast ring + TerminalGrid have real content. Then measure
// echo latency the same way Test 1 does. If echo-after-load is
// markedly slower than Test 1, the WS/daemon path has a cost per
// accumulated frame.

async function testStalePaneEcho(): Promise<void> {
  console.log('\n[Test 3] Echo latency after 5KB of prior output (stale pane)')
  console.log('-'.repeat(72))

  const { sessionId } = await spawnSession('bash', [])
  const sub = openSubscription(sessionId)
  await sub.opened
  await new Promise((r) => setTimeout(r, 300))

  // Fill with ~5KB of output.
  await writeToSession(sessionId, 'yes pad | head -n 500\n')
  await new Promise((r) => setTimeout(r, 800))
  const framesAfterLoad = sub.frames.length
  console.log(`  Accumulated ${framesAfterLoad} frames during load.`)

  const results: number[] = []
  for (let i = 0; i < 20; i++) {
    const char = String.fromCharCode(0x41 + i) // 'A'..'T'
    const byte = char.charCodeAt(0)
    const preCount = sub.frames.length
    const startT = performance.now()
    await writeToSession(sessionId, char)
    const deadline = performance.now() + 2_000
    let echo: number | null = null
    while (performance.now() < deadline) {
      for (let j = preCount; j < sub.frames.length; j++) {
        const env = sub.frames[j].env
        if (
          env.event === 'session:frame' &&
          env.payload.frame === 'Text' &&
          (env.payload.data as any).bytes.includes(byte)
        ) {
          echo = sub.frames[j].arrivedAt - startT
          break
        }
      }
      if (echo !== null) break
      await new Promise((r) => setTimeout(r, 1))
    }
    if (echo !== null) results.push(echo)
    await new Promise((r) => setTimeout(r, 30))
  }
  sub.close()

  if (results.length > 0) {
    const p50 = percentile(results, 0.5)
    const p95 = percentile(results, 0.95)
    console.log(
      `  Post-load echo: n=${results.length}  p50=${fmtMs(p50)}  p95=${fmtMs(p95)}`,
    )
  }
}

// ── Driver ──────────────────────────────────────────────────────────

async function main() {
  console.log('Kessel UI-layer bench — measuring daemon→WS→browser latency')
  console.log(`Daemon: 127.0.0.1:${port}`)

  try {
    await testWsRoundTrip()
    await testSustainedThroughput()
    await testStalePaneEcho()
  } catch (e) {
    console.error('\n❌ Bench failed:', e)
    process.exit(1)
  }
  console.log('\n✅ All tests completed.')
  process.exit(0)
}

main()
