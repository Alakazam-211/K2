// A fake K2 gateway for the server SDK's unit tests: a loopback HTTP
// server that records requests and answers from a handler, plus a minimal
// WebSocket upgrade (text frames and close codes only). Never a real K2.

import { createHash } from 'node:crypto'
import { createServer, type IncomingMessage, type Server } from 'node:http'
import type { AddressInfo, Socket } from 'node:net'

export interface Seen {
  method: string
  url: string
  headers: IncomingMessage['headers']
  body: string
}

export interface Answer {
  status: number
  body: unknown
  headers?: Record<string, string>
}

export interface WsConn {
  url: string
  send(frame: unknown): void
  close(code: number): void
  /** Resolves when the client sends its close frame. */
  closedByClient: Promise<void>
}

export interface FakeGateway {
  url: string
  seen: Seen[]
  sockets: WsConn[]
  /** Called for each upgrade; return a status to refuse it (no 101). */
  onUpgrade: (url: string) => number | null
  onSocket: (c: WsConn) => void
  stop(): Promise<void>
}

const GUID = '258EAFA5-E914-47DA-95CA-C5AB0DC85B11'

function frame(opcode: number, payload: Buffer): Buffer {
  const len = payload.length
  if (len < 126) return Buffer.concat([Buffer.from([0x80 | opcode, len]), payload])
  const head = Buffer.alloc(4)
  head[0] = 0x80 | opcode
  head[1] = 126
  head.writeUInt16BE(len, 2)
  return Buffer.concat([head, payload])
}

export async function startFake(handler: (s: Seen) => Answer): Promise<FakeGateway> {
  const seen: Seen[] = []
  const sockets: WsConn[] = []
  const open = new Set<Socket>()
  const server: Server = createServer((req, res) => {
    let body = ''
    req.on('data', (d: Buffer) => (body += d.toString('utf8')))
    req.on('end', () => {
      const s: Seen = { method: req.method ?? '', url: req.url ?? '', headers: req.headers, body }
      seen.push(s)
      const a = handler(s)
      res.writeHead(a.status, { 'content-type': 'application/json', ...(a.headers ?? {}) })
      res.end(typeof a.body === 'string' ? a.body : JSON.stringify(a.body))
    })
  })
  server.on('connection', (c: Socket) => {
    open.add(c)
    c.on('close', () => open.delete(c))
  })
  const fake: FakeGateway = {
    url: '',
    seen,
    sockets,
    onUpgrade: () => null,
    onSocket: () => {},
    stop: () =>
      new Promise<void>((resolve) => {
        for (const c of open) c.destroy()
        server.close(() => resolve())
      }),
  }
  server.on('upgrade', (req: IncomingMessage, sock: Socket) => {
    const url = req.url ?? ''
    const refuse = fake.onUpgrade(url)
    if (refuse !== null) {
      sock.end(`HTTP/1.1 ${refuse} Refused\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{"error":"refused"}`)
      return
    }
    const key = String(req.headers['sec-websocket-key'] ?? '')
    const accept = createHash('sha1').update(key + GUID).digest('base64')
    sock.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`)
    let closedResolve: () => void = () => {}
    const closedByClient = new Promise<void>((r) => (closedResolve = r))
    sock.on('data', (d: Buffer) => {
      if (((d[0] ?? 0) & 0x0f) === 8) {
        closedResolve()
        // Echo the close unless this side already closed (a 4008 test).
        if (!sock.writableEnded) sock.end(frame(8, Buffer.from([0x03, 0xe8])))
      }
    })
    const conn: WsConn = {
      url,
      send: (f) => sock.write(frame(1, Buffer.from(typeof f === 'string' ? f : JSON.stringify(f), 'utf8'))),
      close: (code) => {
        const p = Buffer.alloc(2)
        p.writeUInt16BE(code, 0)
        sock.end(frame(8, p))
      },
      closedByClient,
    }
    sockets.push(conn)
    fake.onSocket(conn)
  })
  await new Promise<void>((r) => server.listen(0, '127.0.0.1', () => r()))
  fake.url = `http://127.0.0.1:${(server.address() as AddressInfo).port}`
  return fake
}

/** Wait until `f()` is true (polling), or fail loudly after `ms`. */
export async function until(what: string, f: () => boolean, ms = 5000): Promise<void> {
  const end = Date.now() + ms
  while (!f()) {
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`)
    await new Promise((r) => setTimeout(r, 10))
  }
}
