import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

function shippedCsp(): string {
  const path = resolve(__dirname, '../../../src-tauri/tauri.conf.json')
  const conf = JSON.parse(readFileSync(path, 'utf8')) as {
    app?: { security?: { csp?: string } }
  }
  const csp = conf.app?.security?.csp
  if (typeof csp !== 'string' || !csp.trim()) {
    throw new Error(`shipped CSP missing in ${path}`)
  }
  return csp
}

function connectSrc(csp: string): string[] {
  const dir = csp
    .split(';')
    .map((s) => s.trim())
    .find((s) => s.startsWith('connect-src '))
  if (!dir) throw new Error(`no connect-src in CSP: ${csp}`)
  return dir.slice('connect-src '.length).split(/\s+/).filter(Boolean)
}

/** WebView-shaped connect-src match for an http(s)/ws(s) URL. */
function connectSrcAllows(sources: string[], url: string): boolean {
  const u = new URL(url)
  const scheme = u.protocol.replace(/:$/, '')
  const host = u.hostname
  const port = u.port || (scheme === 'https' || scheme === 'wss' ? '443' : '80')
  for (const src of sources) {
    if (src === "'self'" || src === '*') return true
    if (src === scheme || src === `${scheme}:`) return true
    let rest = src
    const schemeSep = rest.indexOf('://')
    if (schemeSep === -1) continue
    const srcScheme = rest.slice(0, schemeSep)
    if (srcScheme !== scheme) continue
    rest = rest.slice(schemeSep + 3)
    let srcHost = rest
    let srcPort: string | null = null
    const portSep = rest.lastIndexOf(':')
    if (portSep !== -1 && !rest.slice(portSep + 1).includes(']')) {
      srcHost = rest.slice(0, portSep)
      srcPort = rest.slice(portSep + 1)
    }
    const hostOk =
      srcHost === '*' ||
      srcHost === host ||
      (srcHost.startsWith('*.') && (host === srcHost.slice(2) || host.endsWith(srcHost.slice(1))))
    const portOk = srcPort === null || srcPort === '*' || srcPort === port
    if (hostOk && portOk) return true
  }
  return false
}

describe('shipped Tauri connect-src', () => {
  it('is a valid CSP string that allows LAN HTTP/WS', () => {
    const csp = shippedCsp()
    expect(csp.includes(';')).toBe(true)
    const sources = connectSrc(csp)
    expect(sources.includes('http://*:*')).toBe(true)
    expect(sources.includes('ws://*:*')).toBe(true)
    expect(sources.some((s) => s.includes('127.0.0.1'))).toBe(true)
    expect(sources.some((s) => s.includes('github.com'))).toBe(true)
    expect(sources.some((s) => s.includes('supabase.co'))).toBe(true)
    expect(sources.includes('https://*.k2.dev')).toBe(true)
    expect(sources.includes('wss://*.k2.dev')).toBe(true)
    expect(connectSrcAllows(sources, 'http://192.168.1.50:60710')).toBe(true)
    expect(connectSrcAllows(sources, 'ws://192.168.1.50:60710')).toBe(true)
  })
})
