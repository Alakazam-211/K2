// Test App for tests/linux/apps_guarded_e2e.sh: a small Node server that
// runs INSIDE the guarded container and reports what it can and can't
// reach (escape smoke E1, E3–E10, E13, E14 subset). Never used outside the
// throwaway VM.
import http from "node:http";
import fs from "node:fs";
import net from "node:net";
import dns from "node:dns/promises";
import { execFileSync } from "node:child_process";

const port = Number(process.env.PORT);
const here = new URL(".", import.meta.url).pathname;
const read = (f) => fs.readFileSync(here + f, "utf8").trim();
const TAG = read("tag.txt");
const facts = JSON.parse(read("facts.json")); // {daemonPort, hostIp}
let inflight = 0;

function tcp(host, p, ms = 2000) {
  return new Promise((res) => {
    const s = net.connect({ host, port: p });
    const t = setTimeout(() => { s.destroy(); res("timeout"); }, ms);
    s.on("connect", () => { clearTimeout(t); s.destroy(); res("open"); });
    s.on("error", (e) => { clearTimeout(t); res(e.code || "error"); });
  });
}
function get(url, ms = 3000, headers = {}) {
  return new Promise((res) => {
    const r = http.get(url, { timeout: ms, headers }, (x) => {
      let b = "";
      x.on("data", (d) => (b += d));
      x.on("end", () => res({ status: x.statusCode, body: b.trim() }));
    });
    r.on("timeout", () => { r.destroy(); res({ status: 0, body: "timeout" }); });
    r.on("error", (e) => res({ status: 0, body: e.code || "error" }));
  });
}
// S15: a raw CONNECT through a proxy port → {status, code} (code from the
// proxy's JSON refusal), or {status: 0, err} when the port is shut.
function connectVia(px, target, ms = 5000) {
  return new Promise((res) => {
    const s = net.connect({ host: "127.0.0.1", port: px });
    let b = "";
    const t = setTimeout(() => { s.destroy(); res({ status: 0, err: "timeout" }); }, ms);
    s.on("connect", () => s.write(`CONNECT ${target} HTTP/1.1\r\nHost: ${target}\r\n\r\n`));
    s.on("data", (d) => {
      b += d;
      const i = b.indexOf("\r\n\r\n");
      if (i < 0) return;
      const status = Number(b.split(" ")[1]);
      if (status === 200) { clearTimeout(t); s.destroy(); return res({ status }); }
      const len = Number((b.slice(0, i).match(/Content-Length: (\d+)/) || [0, 0])[1]);
      if (b.length < i + 4 + len) return;
      clearTimeout(t); s.destroy();
      let code = "";
      try { code = JSON.parse(b.slice(i + 4, i + 4 + len)).error.code; } catch (e) { code = "?"; }
      res({ status, code });
    });
    s.on("error", (e) => { clearTimeout(t); res({ status: 0, err: e.code || "error" }); });
  });
}
async function egress(target) {
  const px = Number(new URL(process.env.HTTPS_PROXY || "http://x:0").port);
  let fetched;
  try {
    const r = await fetch(`https://${target.split(":")[0]}/`, { signal: AbortSignal.timeout(8000) });
    fetched = { status: r.status };
  } catch (e) {
    fetched = { status: 0, err: String((e.cause && e.cause.code) || e.name || e).slice(0, 120) };
  }
  return {
    proxyEnv: { https: process.env.HTTPS_PROXY || null, no: process.env.NO_PROXY || null, node: process.env.NODE_USE_ENV_PROXY || null },
    connect: await connectVia(px, target),
    metadata: await connectVia(px, "169.254.169.254:80"),
    loopback: await connectVia(px, `127.0.0.1:${facts.daemonPort}`),
    // Another App's proxy port (never forwarded; the per-App nft rule).
    otherProxy: await tcp("127.0.0.1", px === 38900 ? 38901 : 38900),
    // Speaking HTTP to the proxy as if it were K2.
    asK2: await get(`http://127.0.0.1:${px}/cli/projects/list`),
    fetch: fetched,
  };
}
function attempt(fn) {
  try { return { ok: true, out: String(fn()).trim().slice(0, 200) }; } catch (e) { return { ok: false, err: String(e.code || e.status || e.message).slice(0, 200) }; }
}
function status(field) {
  const l = read0("/proc/self/status").split("\n").find((x) => x.startsWith(field + ":"));
  return l ? l.split(":")[1].trim() : null;
}
function read0(p) { try { return fs.readFileSync(p, "utf8"); } catch (e) { return ""; } }

// S14: the App's own gateway link, with its App pass: `/gw/me` names the
// link (`gw:<host gateway port>`) the daemon accepted the call on.
async function gateway() {
  const url = process.env.K2_GATEWAY_URL || "";
  const pass = process.env.K2_PASS_FILE ? read0(process.env.K2_PASS_FILE).trim() : "";
  const r = await get(`${url}/gw/me`, 3000, { authorization: `Bearer ${pass}` });
  let link = null;
  try { link = JSON.parse(r.body).link; } catch (e) { link = null; }
  return { url, status: r.status, link, body: r.status === 200 ? undefined : r.body.slice(0, 200) };
}

async function probe() {
  return {
    tag: TAG,
    uid: process.getuid(),
    env: Object.keys(process.env).sort(),
    k2AppId: process.env.K2_APP_ID,
    k2AppName: process.env.K2_APP_NAME,
    nodeOptions: process.env.NODE_OPTIONS,
    greeting: process.env.GREETING || null,
    // E1: the daemon user's home and K2's state are not there at all.
    daemonHome: fs.existsSync("/home/k2"),
    ownerToken: fs.existsSync("/home/k2/.k2/daemon.token"),
    k2appHome: fs.existsSync("/var/lib/k2app"),
    // E13: the Seed and the root are read-only; data is writable.
    writeApp: attempt(() => fs.writeFileSync("/app/pwn", "x")),
    writeRoot: attempt(() => fs.writeFileSync("/pwn", "x")),
    writeData: attempt(() => { fs.writeFileSync("/data/ok", TAG); return fs.readFileSync("/data/ok", "utf8"); }),
    // Floor: no capabilities, no new privileges, seccomp filter on.
    capEff: status("CapEff"),
    capBnd: status("CapBnd"),
    noNewPrivs: status("NoNewPrivs"),
    seccomp: status("Seccomp"),
    // E5: no user namespaces.
    unshareU: attempt(() => execFileSync("unshare", ["-U", "true"], { stdio: "pipe" })),
    unshareR: attempt(() => execFileSync("unshare", ["-r", "true"], { stdio: "pipe" })),
    // E3: host loopback ports other than the App's link.
    daemonPort: await tcp("127.0.0.1", facts.daemonPort),
    hostSsh: await tcp("127.0.0.1", 22),
    hostIpSsh: await tcp(facts.hostIp, 22),
    hostContainersInternal: await tcp("host.containers.internal", 22),
    // E7/E8: no direct DNS or internet.
    internet: await tcp("1.1.1.1", 443),
    dns: await dns.lookup("example.com").then((r) => r.address, (e) => e.code || "error"),
    // The App's own gateway link (pasta -T 8730:<gw> + the per-App nft rule).
    gateway: await gateway(),
    // Another App's gateway port (never forwarded into this App).
    otherGateway: await get(`http://127.0.0.1:${process.env.K2_APP_NAME === "esc-a" ? 38601 : 38600}/`),
    secretsDir: attempt(() => fs.readdirSync(process.env.K2_SECRETS_DIR || "/run/secrets").join(",")),
    passFile: process.env.K2_PASS_FILE ? attempt(() => fs.readFileSync(process.env.K2_PASS_FILE, "utf8")) : null,
    procPids: fs.readdirSync("/proc").filter((x) => /^\d+$/.test(x)).length,
  };
}

const server = http.createServer(async (req, res) => {
  inflight++;
  try {
    const u = new URL(req.url, "http://x");
    if (u.pathname === "/healthz") {
      res.end(`ok ${TAG}\n`);
    } else if (u.pathname === "/probe") {
      res.setHeader("content-type", "application/json");
      res.end(JSON.stringify(await probe()));
    } else if (u.pathname === "/egress") {
      res.setHeader("content-type", "application/json");
      res.end(JSON.stringify(await egress(u.searchParams.get("target") || "example.com:443")));
    } else if (u.pathname === "/flood") {
      for (let i = 0; i < 20000; i++) console.log(`flood ${TAG} line ${i}`);
      res.end("flooded\n");
    } else if (u.pathname === "/slow") {
      await new Promise((r) => setTimeout(r, Number(u.searchParams.get("ms") || 1000)));
      res.end(`slow ${TAG}\n`);
    } else {
      res.end(`${TAG}\n`);
    }
  } finally {
    inflight--;
  }
});
server.listen(port, "0.0.0.0", () => console.log(`esc ${TAG} up on ${port}`));
process.on("SIGTERM", () => {
  console.log(`esc ${TAG} SIGTERM, draining ${inflight}`);
  server.close(() => { console.log(`esc ${TAG} drained`); process.exit(0); });
});
