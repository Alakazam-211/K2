// Low-latency VUI for an H.264 SPS (pure; no DOM).
//
// VideoToolbox's real-time encoder (the sim helper, the daemon, deskd through
// kessel-video) writes an SPS with no VUI at all. Without the VUI's bitstream
// restriction a decoder must assume frames may be reordered and may hold up
// to the level's whole DPB before it outputs anything, and WebKit's decoder
// does: on z3mbpZ (11-10) the Mobile tab drew 2–4 fps while the daemon sent
// 55. This adds the restriction WebRTC's SPS rewriter adds:
// max_num_reorder_frames = 0 and max_dec_frame_buffering = max_num_ref_frames,
// so each frame is output as soon as it is decoded.
//
// Only an SPS with no VUI is rewritten; one that has a VUI is left as it is.
// Anything unexpected (a truncated SPS, an unknown profile path) leaves the
// access unit untouched.

const NAL_SPS = 7

class BitReader {
  private pos = 0
  constructor(private readonly b: Uint8Array) {}
  get offset(): number {
    return this.pos
  }
  bit(): number {
    const byte = this.pos >> 3
    if (byte >= this.b.length) throw new Error('SPS ends early')
    const v = (this.b[byte] >> (7 - (this.pos & 7))) & 1
    this.pos += 1
    return v
  }
  bits(n: number): number {
    let v = 0
    for (let i = 0; i < n; i++) v = v * 2 + this.bit()
    return v
  }
  ue(): number {
    let zeros = 0
    while (this.bit() === 0) {
      zeros += 1
      if (zeros > 31) throw new Error('bad Exp-Golomb code')
    }
    return 2 ** zeros - 1 + this.bits(zeros)
  }
  se(): number {
    const k = this.ue()
    return k % 2 === 1 ? (k + 1) / 2 : -(k / 2)
  }
}

class BitWriter {
  private readonly out: number[] = []
  private cur = 0
  private n = 0
  bit(v: number): void {
    this.cur = (this.cur << 1) | (v & 1)
    this.n += 1
    if (this.n === 8) {
      this.out.push(this.cur)
      this.cur = 0
      this.n = 0
    }
  }
  bits(v: number, n: number): void {
    for (let i = n - 1; i >= 0; i--) this.bit(Math.floor(v / 2 ** i) % 2)
  }
  ue(v: number): void {
    const x = v + 1
    const len = Math.floor(Math.log2(x))
    for (let i = 0; i < len; i++) this.bit(0)
    this.bits(x, len + 1)
  }
  /** rbsp_trailing_bits: a 1, then zeros to the byte boundary. */
  trailing(): Uint8Array {
    this.bit(1)
    while (this.n !== 0) this.bit(0)
    return Uint8Array.from(this.out)
  }
}

/** EBSP → RBSP (drop each emulation-prevention 0x03 after two zeros). */
function unescape(ebsp: Uint8Array): Uint8Array {
  const out: number[] = []
  let zeros = 0
  for (const x of ebsp) {
    if (zeros >= 2 && x === 3) {
      zeros = 0
      continue
    }
    out.push(x)
    zeros = x === 0 ? zeros + 1 : 0
  }
  return Uint8Array.from(out)
}

/** RBSP → EBSP (a 0x03 before any 00..03 byte that follows two zeros). */
function escape(rbsp: Uint8Array): Uint8Array {
  const out: number[] = []
  let zeros = 0
  for (const x of rbsp) {
    if (zeros >= 2 && x <= 3) {
      out.push(3)
      zeros = 0
    }
    out.push(x)
    zeros = x === 0 ? zeros + 1 : 0
  }
  return Uint8Array.from(out)
}

const HIGH_PROFILES = new Set([100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135])

function skipScalingList(r: BitReader, size: number): void {
  let last = 8
  let next = 8
  for (let j = 0; j < size; j++) {
    if (next !== 0) next = (last + r.se() + 256) % 256
    last = next === 0 ? last : next
  }
}

/** Where `vui_parameters_present_flag` sits in an SPS RBSP (bit offset, after
 *  the NAL header byte), and `max_num_ref_frames`. */
function findVuiFlag(rbsp: Uint8Array): { at: number; vui: boolean; refs: number } {
  const r = new BitReader(rbsp)
  r.bits(8) // NAL header
  const profile = r.bits(8)
  r.bits(8) // constraint flags
  r.bits(8) // level
  r.ue() // seq_parameter_set_id
  if (HIGH_PROFILES.has(profile)) {
    const chroma = r.ue()
    if (chroma === 3) r.bit()
    r.ue() // bit_depth_luma
    r.ue() // bit_depth_chroma
    r.bit() // qpprime_y_zero_transform_bypass
    if (r.bit()) {
      for (let i = 0; i < (chroma !== 3 ? 8 : 12); i++) if (r.bit()) skipScalingList(r, i < 6 ? 16 : 64)
    }
  }
  r.ue() // log2_max_frame_num
  const poc = r.ue()
  if (poc === 0) r.ue()
  else if (poc === 1) {
    r.bit()
    r.se()
    r.se()
    const n = r.ue()
    for (let i = 0; i < n; i++) r.se()
  }
  const refs = r.ue()
  r.bit() // gaps_in_frame_num_allowed
  r.ue() // width in MBs
  r.ue() // height in map units
  if (!r.bit()) r.bit() // frame_mbs_only → mb_adaptive
  r.bit() // direct_8x8_inference
  if (r.bit()) {
    r.ue()
    r.ue()
    r.ue()
    r.ue()
  }
  const at = r.offset
  return { at, vui: r.bit() === 1, refs }
}

/** One SPS NAL (header byte included, EBSP) with the low-latency VUI added,
 *  or the same bytes when it already has a VUI or can't be read. */
export function spsWithLowLatencyVui(sps: Uint8Array): Uint8Array {
  if (sps.length < 4 || (sps[0] & 0x1f) !== NAL_SPS) return sps
  try {
    const rbsp = unescape(sps)
    const { at, vui, refs } = findVuiFlag(rbsp)
    if (vui) return sps
    const r = new BitReader(rbsp)
    const w = new BitWriter()
    for (let i = 0; i < at; i++) w.bit(r.bit())
    w.bit(1) // vui_parameters_present_flag
    w.bit(0) // aspect_ratio_info_present
    w.bit(0) // overscan_info_present
    w.bit(0) // video_signal_type_present
    w.bit(0) // chroma_loc_info_present
    w.bit(0) // timing_info_present
    w.bit(0) // nal_hrd_parameters_present
    w.bit(0) // vcl_hrd_parameters_present
    w.bit(0) // pic_struct_present
    w.bit(1) // bitstream_restriction_flag
    w.bit(1) // motion_vectors_over_pic_boundaries
    w.ue(2) // max_bytes_per_pic_denom (the default)
    w.ue(1) // max_bits_per_mb_denom (the default)
    w.ue(16) // log2_max_mv_length_horizontal
    w.ue(16) // log2_max_mv_length_vertical
    w.ue(0) // max_num_reorder_frames: no B-frames
    w.ue(Math.max(1, refs)) // max_dec_frame_buffering
    return escape(w.trailing())
  } catch {
    return sps
  }
}

/** An Annex B access unit with every SPS given the low-latency VUI (4-byte
 *  start codes out). The same array when no SPS changed. */
export function withLowLatencyVui(au: Uint8Array): Uint8Array {
  const starts: [number, number][] = [] // [start code at, NAL at]
  for (let i = 0; i + 3 <= au.length; ) {
    if (au[i] === 0 && au[i + 1] === 0 && au[i + 2] === 1) {
      const sc = i > 0 && au[i - 1] === 0 ? i - 1 : i
      starts.push([sc, i + 3])
      i += 3
    } else i += 1
  }
  if (!starts.some(([, at]) => (au[at] & 0x1f) === NAL_SPS)) return au
  const parts: Uint8Array[] = []
  let changed = false
  starts.forEach(([, at], k) => {
    const end = k + 1 < starts.length ? starts[k + 1][0] : au.length
    let nal = au.subarray(at, end)
    if ((nal[0] & 0x1f) === NAL_SPS) {
      const fixed = spsWithLowLatencyVui(nal)
      if (fixed !== nal) changed = true
      nal = fixed
    }
    parts.push(Uint8Array.of(0, 0, 0, 1), nal)
  })
  if (!changed) return au
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0))
  let o = 0
  for (const p of parts) {
    out.set(p, o)
    o += p.length
  }
  return out
}

/** The SPS's bitstream restriction, for tests and the readout:
 *  `{ reorder, buffering }`, or null without one. */
export function spsRestriction(sps: Uint8Array): { reorder: number; buffering: number } | null {
  const rbsp = unescape(sps)
  const { at, vui } = findVuiFlag(rbsp)
  if (!vui) return null
  const r = new BitReader(rbsp)
  for (let i = 0; i <= at; i++) r.bit()
  if (r.bit()) {
    if (r.bits(8) === 255) r.bits(32)
  }
  if (r.bit()) r.bit()
  if (r.bit()) {
    r.bits(4)
    if (r.bit()) r.bits(24)
  }
  if (r.bit()) {
    r.ue()
    r.ue()
  }
  if (r.bit()) {
    r.bits(32)
    r.bits(32)
    r.bit()
  }
  const hrd = (): void => {
    const cpb = r.ue()
    r.bits(8)
    for (let i = 0; i <= cpb; i++) {
      r.ue()
      r.ue()
      r.bit()
    }
    r.bits(20)
  }
  const nal = r.bit()
  if (nal) hrd()
  const vcl = r.bit()
  if (vcl) hrd()
  if (nal || vcl) r.bit()
  r.bit() // pic_struct_present
  if (!r.bit()) return null
  r.bit()
  r.ue()
  r.ue()
  r.ue()
  r.ue()
  return { reorder: r.ue(), buffering: r.ue() }
}
