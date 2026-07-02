// "k1" — Kessel wire format v1: client decoder.
//
// Mirrors the layout documented in
// `crates/k2-core/src/terminal/grid_wire.rs` (the single source of
// truth). A connection opts in with `&proto=k1` on the grid-WS URL;
// the daemon then sends `snapshot`/`delta` frames as WS BINARY
// messages in this format while every other event (title, bell,
// labels, child_exit, error) stays a JSON text frame. Decoding a k1
// frame produces the EXACT object `JSON.parse` of the equivalent
// JSON frame would have produced (including the absence of the
// `wrapped` key on non-wrapped runs) — TerminalPane consumes both
// paths through the same merge/render code.
//
// Layout (all integers little-endian):
//
//   header   : u8 magic (0x6B, 'k') · u8 format version (1) ·
//              u8 kind (1 = snapshot, 2 = delta)
//
//   str      : u16 byte-len · UTF-8 bytes
//   color    : u32 — 0xFFFF_FFFF = terminal default (null),
//              else 0x00RRGGBB
//   run      : str text · color fg · color bg · u8 style bits
//              (bit0 bold · bit1 italic · bit2 underline ·
//               bit3 inverse · bit4 dim · bit5 strikeout ·
//               bit6 wrapped · bit7 has-cols) ·
//              [u16 cols — only when bit7 set; the run's terminal-
//               column span, present iff it differs from the char
//               count (wide/zero-width content)]
//   row      : u16 run-count · runs
//   rows     : u32 row-count · rows
//   cursor   : u16 row · u16 col · u8 visible (0/1)
//
//   snapshot : header · str paneId · u16 cols · u16 rows ·
//              u64 version · u32 displayOffset · cursor ·
//              u8 mode bits (bit0 mouseReport · bit1 sgrMouse ·
//                            bit2 altScreen) ·
//              rows grid · rows scrollback
//
//   delta    : header · str paneId · u16 cols · u16 rows ·
//              u64 version · u32 displayOffset · cursor ·
//              u32 damaged-count · per damaged row:
//                (u16 row-index · row) ·
//              rows scrollbackAppended

export interface WireCellRun {
  text: string
  fg: number | null
  bg: number | null
  bold: boolean
  italic: boolean
  underline: boolean
  inverse: boolean
  dim: boolean
  strikeout: boolean
  wrapped?: boolean
  /** Terminal-column span, present only when it differs from the
   *  run's char count (double-width CJK/emoji or zero-width
   *  combining chars). Absent ⇒ one column per char. */
  cols?: number
}

export interface WireCursorSnapshot {
  row: number
  col: number
  visible: boolean
}

export interface WireGridSnapshot {
  paneId: string
  cols: number
  rows: number
  grid: WireCellRun[][]
  scrollback: WireCellRun[][]
  cursor: WireCursorSnapshot
  version: number
  displayOffset: number
  mouseReport: boolean
  sgrMouse: boolean
  altScreen: boolean
}

export interface WireDamagedRow {
  row: number
  runs: WireCellRun[]
}

export interface WireGridDelta {
  paneId: string
  cols: number
  rows: number
  damagedRows: WireDamagedRow[]
  scrollbackAppended: WireCellRun[][]
  cursor: WireCursorSnapshot
  version: number
  displayOffset: number
}

export type WireFrame =
  | { kind: 'snapshot'; payload: WireGridSnapshot }
  | { kind: 'delta'; payload: WireGridDelta }

const WIRE_MAGIC = 0x6b
const WIRE_VERSION = 1
const KIND_SNAPSHOT = 1
const KIND_DELTA = 2

const COLOR_NONE = 0xffff_ffff

const STYLE_BOLD = 1 << 0
const STYLE_ITALIC = 1 << 1
const STYLE_UNDERLINE = 1 << 2
const STYLE_INVERSE = 1 << 3
const STYLE_DIM = 1 << 4
const STYLE_STRIKEOUT = 1 << 5
const STYLE_WRAPPED = 1 << 6
const STYLE_HAS_COLS = 1 << 7

const MODE_MOUSE_REPORT = 1 << 0
const MODE_SGR_MOUSE = 1 << 1
const MODE_ALT_SCREEN = 1 << 2

const utf8 = new TextDecoder('utf-8', { fatal: true })

/** Sequential DataView reader. Throws on truncation — a malformed
 *  frame is a protocol violation and must fail loudly, never yield a
 *  silently-wrong grid. */
class Reader {
  private view: DataView
  private bytes: Uint8Array
  private pos = 0

  constructor(buf: ArrayBuffer) {
    this.view = new DataView(buf)
    this.bytes = new Uint8Array(buf)
  }

  private need(n: number) {
    if (this.pos + n > this.bytes.length) {
      throw new Error(
        `k1 wire decode error: truncated frame, need ${n} bytes at offset ${this.pos}`,
      )
    }
  }

  u8(): number {
    this.need(1)
    return this.view.getUint8(this.pos++)
  }

  u16(): number {
    this.need(2)
    const v = this.view.getUint16(this.pos, true)
    this.pos += 2
    return v
  }

  u32(): number {
    this.need(4)
    const v = this.view.getUint32(this.pos, true)
    this.pos += 4
    return v
  }

  /** u64 → JS number. Grid versions are frame counters — far below
   *  2^53, so the conversion is exact. */
  u64(): number {
    this.need(8)
    const v = this.view.getBigUint64(this.pos, true)
    this.pos += 8
    return Number(v)
  }

  str(): string {
    const len = this.u16()
    this.need(len)
    const s = utf8.decode(this.bytes.subarray(this.pos, this.pos + len))
    this.pos += len
    return s
  }

  color(): number | null {
    const v = this.u32()
    return v === COLOR_NONE ? null : v
  }

  run(): WireCellRun {
    const text = this.str()
    const fg = this.color()
    const bg = this.color()
    const bits = this.u8()
    // The trailing u16 must be consumed BEFORE building the object —
    // it sits in the byte stream whenever bit7 is set.
    const cols = (bits & STYLE_HAS_COLS) !== 0 ? this.u16() : null
    const run: WireCellRun = {
      text,
      fg,
      bg,
      bold: (bits & STYLE_BOLD) !== 0,
      italic: (bits & STYLE_ITALIC) !== 0,
      underline: (bits & STYLE_UNDERLINE) !== 0,
      inverse: (bits & STYLE_INVERSE) !== 0,
      dim: (bits & STYLE_DIM) !== 0,
      strikeout: (bits & STYLE_STRIKEOUT) !== 0,
    }
    // Keys only present when set — matches serde's
    // skip_serializing_if on the JSON path, so deep-equality between
    // the two transports holds.
    if ((bits & STYLE_WRAPPED) !== 0) run.wrapped = true
    if (cols !== null) run.cols = cols
    return run
  }

  row(): WireCellRun[] {
    const n = this.u16()
    const out: WireCellRun[] = new Array(n)
    for (let i = 0; i < n; i++) out[i] = this.run()
    return out
  }

  rows(): WireCellRun[][] {
    const n = this.u32()
    const out: WireCellRun[][] = new Array(n)
    for (let i = 0; i < n; i++) out[i] = this.row()
    return out
  }

  cursor(): WireCursorSnapshot {
    return { row: this.u16(), col: this.u16(), visible: this.u8() !== 0 }
  }
}

/** Decode one k1 binary WS message. Throws on malformed input. */
export function decodeGridFrame(buf: ArrayBuffer): WireFrame {
  const r = new Reader(buf)
  const magic = r.u8()
  if (magic !== WIRE_MAGIC) {
    throw new Error(`k1 wire decode error: bad magic 0x${magic.toString(16)}`)
  }
  const version = r.u8()
  if (version !== WIRE_VERSION) {
    throw new Error(`k1 wire decode error: unsupported wire version ${version}`)
  }
  const kind = r.u8()
  if (kind === KIND_SNAPSHOT) {
    const paneId = r.str()
    const cols = r.u16()
    const rows = r.u16()
    const frameVersion = r.u64()
    const displayOffset = r.u32()
    const cursor = r.cursor()
    const mode = r.u8()
    const grid = r.rows()
    const scrollback = r.rows()
    return {
      kind: 'snapshot',
      payload: {
        paneId,
        cols,
        rows,
        grid,
        scrollback,
        cursor,
        version: frameVersion,
        displayOffset,
        mouseReport: (mode & MODE_MOUSE_REPORT) !== 0,
        sgrMouse: (mode & MODE_SGR_MOUSE) !== 0,
        altScreen: (mode & MODE_ALT_SCREEN) !== 0,
      },
    }
  }
  if (kind === KIND_DELTA) {
    const paneId = r.str()
    const cols = r.u16()
    const rows = r.u16()
    const frameVersion = r.u64()
    const displayOffset = r.u32()
    const cursor = r.cursor()
    const damagedCount = r.u32()
    const damagedRows: WireDamagedRow[] = new Array(damagedCount)
    for (let i = 0; i < damagedCount; i++) {
      const row = r.u16()
      damagedRows[i] = { row, runs: r.row() }
    }
    const scrollbackAppended = r.rows()
    return {
      kind: 'delta',
      payload: {
        paneId,
        cols,
        rows,
        damagedRows,
        scrollbackAppended,
        cursor,
        version: frameVersion,
        displayOffset,
      },
    }
  }
  throw new Error(`k1 wire decode error: unknown frame kind ${kind}`)
}
