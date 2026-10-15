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
function get(url, ms = 3000) {
  return new Promise((res) => {
    const r = http.get(url, { timeout: ms }, (x) => {
      let b = "";
      x.on("data", (d) => (b += d));
      x.on("end", () => res({ status: x.statusCode, body: b.trim() }));
    });
    r.on("timeout", () => { r.destroy(); res({ status: 0, body: "timeout" }); });
    r.on("error", (e) => res({ status: 0, body: e.code || "error" }));
  });
}
function attempt(fn) {
  try { return { ok: true, out: String(fn()).trim().slice(0, 200) }; } catch (e) { return { ok: false, err: String(e.code || e.status || e.message).slice(0, 200) }; }
}
function status(field) {
  const l = read0("/proc/self/status").split("\n").find((x) => x.startsWith(field + ":"));
  return l ? l.split(":")[1].trim() : null;
}
function read0(p) { try { return fs.readFileSync(p, "utf8"); } catch (e) { return ""; } }

async function probe() {
  const gwPort = Number(process.env.K2_PORT);
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
    // The App's own gateway link (pasta -T + the per-App nft rule).
    gateway: await get(`http://127.0.0.1:${gwPort}/`),
    // Another App's gateway port (never forwarded into this App).
    otherGateway: await get(`http://127.0.0.1:${gwPort === 38600 ? 38601 : 38600}/`),
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
