// The must-not-break contract from INSIDE a wall (runtime S23, §14.2):
// an App whose server runs tests/contract/must-not-break.ts through the
// server SDK against its own gateway link. tests/linux/apps_e2e_sdk.sh
// copies it, the SDK and the suite into one folder and deploys it on the
// guarded path (and the open path once lane 1's open engine lands).
// Never used outside a throwaway VM.
//
//   GET /healthz          200 once listening
//   GET /_contract        runs the suite; 200 + the result JSON (ok:false when a check failed)
//   GET /_contract/app    the App's own view (asApp me.get) + the gateway link facts
//
// Node 24 (the guarded base image) runs the TypeScript as is.

import http from 'node:http'
import { readFileSync } from 'node:fs'

import { fromEnv } from './sdk/server/src/index.ts'
import { runMustNotBreak } from './tests/contract/must-not-break.ts'

const port = Number(process.env.PORT)
const fixture = JSON.parse(readFileSync(new URL('./fixture.json', import.meta.url), 'utf8'))
const k2 = fromEnv()
let running = null

function send(res, status, body) {
  res.writeHead(status, { 'content-type': 'application/json' })
  res.end(JSON.stringify(body))
}

http
  .createServer(async (req, res) => {
    try {
      if (req.url === '/healthz' || req.url === '/') return send(res, 200, { ok: true })
      if (req.url === '/_contract') {
        running ??= runMustNotBreak(k2, fixture, process.env).finally(() => {
          running = null
        })
        return send(res, 200, await running)
      }
      if (req.url === '/_contract/app') {
        const me = await k2.asApp().me.get()
        return send(res, 200, { me, gateway: k2.gatewayUrl, appId: k2.appId, appName: k2.appName })
      }
      return send(res, 404, { error: 'not here' })
    } catch (e) {
      return send(res, 500, { error: String(e?.message ?? e), code: e?.code ?? null })
    }
  })
  .listen(port, () => console.log(`sdk contract app up on ${port}`))
