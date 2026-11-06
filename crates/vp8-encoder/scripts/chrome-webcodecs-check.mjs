// Decodes an IVF file of VP8 frames in headless Chromium with WebCodecs and
// checks that every frame is byte-identical to vp8-decoder's output (the
// `.dec.yuv` file the encode_y4m example writes beside the IVF).
//
//   node chrome-webcodecs-check.mjs OUT.ivf [OUT.ivf ...]
//
// Needs the `playwright` package and its Chromium (npx playwright install chromium).

import { chromium } from "playwright";
import { createServer } from "node:http";
import { readFileSync } from "node:fs";
import { createHash } from "node:crypto";

const files = process.argv.slice(2);
if (files.length === 0) {
  console.error("usage: chrome-webcodecs-check.mjs OUT.ivf [...]");
  process.exit(2);
}

const page_html = `<!doctype html><meta charset="utf-8"><title>vp8 check</title>
<script>
async function decodeIvf(url) {
  const buf = new Uint8Array(await (await fetch(url)).arrayBuffer());
  const dv = new DataView(buf.buffer);
  const headerLen = dv.getUint16(6, true);
  const frames = [];
  for (let pos = headerLen; pos + 12 <= buf.length; ) {
    const size = dv.getUint32(pos, true);
    frames.push(buf.subarray(pos + 12, pos + 12 + size));
    pos += 12 + size;
  }
  const outputs = [];
  let failure = null;
  const decoder = new VideoDecoder({
    output: (frame) => outputs.push(frame),
    error: (e) => { failure = String(e); },
  });
  decoder.configure({ codec: "vp8" });
  frames.forEach((data, i) => {
    const key = (data[0] & 1) === 0;
    decoder.decode(new EncodedVideoChunk({ type: key ? "key" : "delta", timestamp: i * 33333, data }));
  });
  await decoder.flush();
  if (failure) return { error: failure };
  const hashes = [];
  let format = null;
  for (const frame of outputs) {
    format = frame.format;
    const bytes = new Uint8Array(frame.allocationSize());
    await frame.copyTo(bytes);
    frame.close();
    const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
    hashes.push(Array.from(digest, (b) => b.toString(16).padStart(2, "0")).join(""));
  }
  return { frames: frames.length, format, hashes };
}
</script>`;

const server = createServer((req, res) => {
  if (req.url === "/") {
    res.writeHead(200, { "content-type": "text/html" });
    res.end(page_html);
    return;
  }
  const i = Number(req.url.slice(1));
  if (Number.isInteger(i) && files[i]) {
    res.writeHead(200, { "content-type": "application/octet-stream" });
    res.end(readFileSync(files[i]));
    return;
  }
  res.writeHead(404);
  res.end();
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const port = server.address().port;

const browser = await chromium.launch();
const page = await browser.newPage();
await page.goto(`http://127.0.0.1:${port}/`);
console.log(`[chrome] ${browser.version()}`);
let failed = false;
for (let i = 0; i < files.length; i++) {
  const result = await page.evaluate((u) => decodeIvf(u), `/${i}`);
  if (result.error) {
    console.log(`[chrome] ${files[i]}: decode error: ${result.error}`);
    failed = true;
    continue;
  }
  const ours = readFileSync(`${files[i]}.dec.yuv`);
  const frameLen = ours.length / result.frames;
  let mismatches = 0;
  for (let f = 0; f < result.frames; f++) {
    const h = createHash("sha256").update(ours.subarray(f * frameLen, (f + 1) * frameLen)).digest("hex");
    if (h !== result.hashes[f]) mismatches++;
  }
  const ok = result.hashes.length === result.frames && mismatches === 0;
  failed ||= !ok;
  console.log(
    `[chrome] ${files[i]}: ${result.frames} frames, ${result.hashes.length} decoded by WebCodecs (${result.format}), ` +
      (ok ? "all identical to vp8-decoder" : `${mismatches} differ from vp8-decoder`),
  );
}
await browser.close();
server.close();
process.exit(failed ? 1 : 0);
