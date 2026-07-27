# Client Connect reconnection flap (GH#57) — fixed in 0.40.68

**Status:** Fixed in **0.40.68**  
**Issue:** https://github.com/Alakazam-211/K2/issues/57  
**Related:** 0.40.48 connection-resilience / poisoned-pool wedge; `reconnect-wedge-watchdog.md`

## Symptom

After a brief drop while signed into a hosted server, the desktop app can
loop on **"Reconnecting to \<server\>"** even though the server and tunnel
are healthy. Quit + relaunch cleared it before this fix.

## What it was *not*

Not a flapping frpc/frps tunnel. Field forensics (scout.k2.dev, 2026-07-27)
showed stable external `/boot-status` while the **client** repeatedly
aborted E2E TLS handshakes (`tls handshake eof` on the daemon) and
thrashed session-event WebSocket attach/detach.

## Root cause (short)

1. The 0.40.48 **wedge detector** only counted webview boot probes that
   failed as **HTTP** errors. GH#57 failures arrived as **network** errors
   (mid-handshake client close), so the detector never escalated to a cold
   rebuild of the webview connection path.
2. A single intermittent successful probe could reset the failure clock
   ("healthy for a split second"), so a continuous failure window never
   accumulated.
3. Session-event reconnect could open a **new** WebSocket without closing
   the previous one → dual dials and handshake abort noise.

## Fix (0.40.68)

- Treat **any** non-ok webview boot probe (http or network) as part of the
  wedge failure run; require a short streak of consecutive oks to clear it.
- **Flap detector:** many reconnecting-banner surfaces in a short window,
  with the out-of-webview arbiter still seeing the host ready → same
  escalate path (auto-reload once, then "Restart K2" if needed).
- Close the prior WebSocket (and detach handlers) before redialing
  session-events / active-state / tab-events sockets.
- Daemon E2E logs include `peer=` on connection end for easier diagnosis.

## If you still see it

1. Update to **0.40.68+**.
2. If an older build: full quit + relaunch (clears the webview network
   process / connection pool).
3. Confirm the *server* is healthy with
   `curl -fsS https://<your-subdomain>.k2.dev/boot-status` — if that flaps,
   the problem is tunnel/host, not this client path.
