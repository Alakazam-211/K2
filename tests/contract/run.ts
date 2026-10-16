// Run the must-not-break suite from outside a wall (the --cmd path):
//
//   K2_GATEWAY_URL=http://127.0.0.1:<daemon port> K2_APP_ID=<svc or empty> \
//     node tests/contract/run.ts <fixture.json> [--allow-not-run]
//
// Node 22.18+ runs the TypeScript as is. Prints the result JSON; exits 1 on
// any failed check, and on any check that couldn't run unless
// --allow-not-run (a Mac has no workspace Postgres, so k2 db can't run
// there; on Linux the db checks must run).

import { readFileSync } from 'node:fs'

import { fromEnv } from '../../sdk/server/src/index.ts'
import { runMustNotBreak, type Fixture } from './must-not-break.ts'

const args = process.argv.slice(2)
const file = args.find((a) => !a.startsWith('--'))
if (!file) {
  console.error('usage: node tests/contract/run.ts <fixture.json> [--allow-not-run]')
  process.exit(2)
}
const fx = JSON.parse(readFileSync(file, 'utf8')) as Fixture
const k2 = fromEnv()
const r = await runMustNotBreak(k2, fx, process.env)
console.log(JSON.stringify(r, null, 2))
for (const f of r.failed) console.error(`FAIL ${f.name}: ${f.detail}`)
for (const n of r.notRun) console.error(`NOT RUN ${n.name}: ${n.why}`)
const notRunFails = r.notRun.length > 0 && !args.includes('--allow-not-run')
if (!r.ok || notRunFails) process.exit(1)
console.error(`ok: ${r.passed} checks`)
