// 0.46.6 relay default (PRD relay-default-and-ptr-guard D8): the desktop
// never pins a relay address. A subdomain bind sends none, the Advanced
// Save sends one only after the person edited it, and no relay IP is baked
// into the renderer.

import { describe, it, expect, vi } from 'vitest'
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }))

import { bindConfigBody, manualConfigBody } from './K2ConnectSection'

describe('K2 Connect relay address (0.46.6)', () => {
  it('a subdomain bind posts no serverAddr/serverPort (PRD test 22)', () => {
    const body = bindConfigBody({ label: 'rosson', tunnel_token: 'k2c_tok' }, 'dev-1', 'MacIntel')
    expect(body).toEqual({
      subdomain: 'rosson',
      token: 'k2c_tok',
      deviceId: 'dev-1',
      deviceLabel: 'MacIntel',
    })
    expect(Object.keys(body)).not.toContain('serverAddr')
    expect(Object.keys(body)).not.toContain('serverPort')
  })

  it('Save sends the relay only after an edit (PRD test 23)', () => {
    const shown = { serverAddr: '178.156.232.105', serverPort: '7000', subdomain: ' rosson ', token: '' }
    const untouched = manualConfigBody({ ...shown, relayEdited: false })
    expect(untouched).toEqual({ ok: true, body: { subdomain: 'rosson' } })

    const edited = manualConfigBody({ ...shown, serverAddr: ' relay.example.net ', relayEdited: true })
    expect(edited).toEqual({
      ok: true,
      body: { subdomain: 'rosson', serverAddr: 'relay.example.net', serverPort: 7000 },
    })

    const withToken = manualConfigBody({ ...shown, token: ' k2c_new ', relayEdited: false })
    expect(withToken).toEqual({ ok: true, body: { subdomain: 'rosson', token: 'k2c_new' } })
  })

  it('an edited empty relay field sends nothing for it; a bad port is refused', () => {
    expect(
      manualConfigBody({ serverAddr: '', serverPort: '', subdomain: 'a', token: '', relayEdited: true }),
    ).toEqual({ ok: true, body: { subdomain: 'a' } })
    for (const port of ['0', '65536', 'x', '7.5']) {
      expect(
        manualConfigBody({ serverAddr: 'h', serverPort: port, subdomain: 'a', token: '', relayEdited: true }),
      ).toEqual({ ok: false, error: 'Server port must be 1–65535' })
    }
  })

  it('no relay IP is baked into src/renderer outside tests (PRD test 24)', () => {
    const renderer = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..')
    const offenders: string[] = []
    const walk = (dir: string): void => {
      for (const name of readdirSync(dir)) {
        const p = join(dir, name)
        if (statSync(p).isDirectory()) {
          walk(p)
        } else if (/\.(ts|tsx)$/.test(name) && !/\.test\.tsx?$/.test(name)) {
          const src = readFileSync(p, 'utf8')
          for (const ip of ['178.156.232.105', '40.160.53.25']) {
            if (src.includes(ip)) offenders.push(`${p}: ${ip}`)
          }
        }
      }
    }
    walk(renderer)
    expect(renderer.endsWith(join('src', 'renderer'))).toBe(true)
    expect(offenders).toEqual([])
  })
})
