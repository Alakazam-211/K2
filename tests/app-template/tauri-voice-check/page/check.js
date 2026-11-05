// TEST ONLY: what a call needs from the webview, on the App's own origin.
// Each check records what it saw (errors included); main.rs decides.
'use strict'

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))
const within = (p, ms, what) => Promise.race([p, sleep(ms).then(() => { throw new Error(`timeout: ${what}`) })])
const err = (e) => `${(e && e.name) || 'Error'}: ${(e && e.message) || e}`

async function wasm() {
  try {
    const { instance } = await WebAssembly.instantiateStreaming(fetch('/add.wasm'))
    return instance.exports.add(20, 22)
  } catch (e) {
    return err(e)
  }
}

async function worklet() {
  const out = {}
  try {
    const bytes = await (await fetch('/add.wasm')).arrayBuffer()
    const ctx = new OfflineAudioContext(1, 128 * 64, 48000)
    await ctx.audioWorklet.addModule('/noise-worklet.js')
    const node = new AudioWorkletNode(ctx, 'k2-noise-check', { processorOptions: { bytes } })
    node.port.onmessage = (e) => Object.assign(out, e.data)
    try {
      node.port.postMessage({ module: await WebAssembly.compile(bytes) })
    } catch (e) {
      out.moduleInWorklet = err(e)
    }
    const osc = ctx.createOscillator()
    osc.connect(node)
    node.connect(ctx.destination)
    osc.start()
    const buf = await within(ctx.startRendering(), 10000, 'offline render')
    out.rendered = buf.length
    await sleep(500)
  } catch (e) {
    out.error = err(e)
  }
  return out
}

async function rtc() {
  const out = {
    peerConnection: typeof RTCPeerConnection === 'function',
    scriptTransform: typeof RTCRtpScriptTransform === 'function',
    encodedStreams: typeof RTCRtpSender !== 'undefined' && 'createEncodedStreams' in RTCRtpSender.prototype,
    transformFrames: {},
  }
  if (!out.peerConnection) return out
  try {
    const pc1 = new RTCPeerConnection()
    const pc2 = new RTCPeerConnection()
    pc1.onicecandidate = (e) => e.candidate && pc2.addIceCandidate(e.candidate)
    pc2.onicecandidate = (e) => e.candidate && pc1.addIceCandidate(e.candidate)
    const ac = new AudioContext()
    const osc = ac.createOscillator()
    const dest = ac.createMediaStreamDestination()
    osc.connect(dest)
    osc.start()
    const sender = pc1.addTrack(dest.stream.getAudioTracks()[0], dest.stream)
    const workers = []
    const transform = (side) => {
      const w = new Worker('/e2ee-worker.js')
      w.onmessage = (e) => { out.transformFrames[e.data.side] = e.data.frames }
      workers.push(w)
      return new RTCRtpScriptTransform(w, { side })
    }
    if (out.scriptTransform) sender.transform = transform('send')
    pc2.ontrack = (e) => {
      if (out.scriptTransform) e.receiver.transform = transform('recv')
    }
    const pong = new Promise((resolve) => {
      pc2.ondatachannel = (e) => { e.channel.onmessage = (m) => e.channel.send(m.data === 'ping' ? 'pong' : 'bad') }
      const dc = pc1.createDataChannel('k2')
      dc.onopen = () => dc.send('ping')
      dc.onmessage = (m) => resolve(m.data)
    })
    const offer = await pc1.createOffer()
    out.offerHasAudio = offer.sdp.includes('m=audio')
    await pc1.setLocalDescription(offer)
    await pc2.setRemoteDescription(offer)
    const answer = await pc2.createAnswer()
    await pc2.setLocalDescription(answer)
    await pc1.setRemoteDescription(answer)
    out.dataChannel = await within(pong, 15000, 'data channel')
    const t0 = Date.now()
    await sleep(10000)
    out.callMs = Date.now() - t0
    let packets = 0
    ;(await pc2.getStats()).forEach((s) => { if (s.type === 'inbound-rtp' && s.kind === 'audio') packets += s.packetsReceived || 0 })
    out.rtpPackets = packets
    out.audioContextForCall = ac.state
    pc1.close()
    pc2.close()
    workers.forEach((w) => w.terminate())
  } catch (e) {
    out.error = err(e)
  }
  return out
}

async function mic() {
  if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) return { ok: false, error: 'no mediaDevices' }
  try {
    const s = await within(navigator.mediaDevices.getUserMedia({ audio: true }), 30000, 'getUserMedia (a person answers the macOS prompt)')
    const label = s.getAudioTracks()[0] && s.getAudioTracks()[0].label
    s.getTracks().forEach((t) => t.stop())
    return { ok: true, label }
  } catch (e) {
    return { ok: false, error: err(e) }
  }
}

async function idb(stamp) {
  const out = {}
  try {
    const db = await within(new Promise((res, rej) => {
      const r = indexedDB.open('k2-voice-check', 1)
      r.onupgradeneeded = () => r.result.createObjectStore('kv')
      r.onsuccess = () => res(r.result)
      r.onerror = () => rej(r.error)
    }), 5000, 'indexedDB.open')
    const tx = (mode) => db.transaction('kv', mode).objectStore('kv')
    out.previous = await new Promise((res) => { const g = tx('readonly').get('stamp'); g.onsuccess = () => res(g.result ?? null); g.onerror = () => res(err(g.error)) })
    await new Promise((res, rej) => { const p = tx('readwrite').put(stamp, 'stamp'); p.onsuccess = res; p.onerror = () => rej(p.error) })
    out.wrote = true
    out.stamp = stamp
  } catch (e) {
    out.error = err(e)
  }
  return out
}

async function timers() {
  let ticks = 0
  const id = setInterval(() => { ticks++ }, 100)
  const t0 = Date.now()
  // A slowed page fires both late; a suspended one never reports at all.
  await sleep(3000)
  clearInterval(id)
  return { ticks, visibility: document.visibilityState, hidden: document.hidden, elapsedMs: Date.now() - t0 }
}

async function audioContext() {
  try {
    const ac = new AudioContext()
    await Promise.race([ac.resume(), sleep(2000)])
    const st = ac.state
    await ac.close()
    return st
  } catch (e) {
    return err(e)
  }
}

async function main() {
  const stamp = new URLSearchParams(location.search).get('stamp') || 'none'
  const r = {
    origin: location.origin,
    secureContext: window.isSecureContext,
    getUserMedia: !!(navigator.mediaDevices && navigator.mediaDevices.getUserMedia),
    userAgent: navigator.userAgent,
  }
  r.timers = await timers()
  r.wasm = await wasm()
  r.worklet = await worklet()
  r.idb = await idb(stamp)
  r.audioContext = await audioContext()
  r.rtc = await rtc()
  r.mic = await mic()
  await fetch('/result/main', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(r) })
  location.href = '/nowasm'
}

main().catch((e) =>
  fetch('/result/main', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ fatal: err(e) }) }),
)
