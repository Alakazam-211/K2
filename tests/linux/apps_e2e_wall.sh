#!/usr/bin/env bash
# Lane 1 phase "Wall" (0.46.1: engine protocol 3, S13 relaxed container,
# K5 `--cmd` -> open path, S16 builds in the wall, S17 resource lines).
# Linux only: it runs AS ROOT INSIDE the throwaway VM that
# tests/linux/apps_guarded_e2e.sh runs in, after phase A has the daemon,
# box prep and the engine up (the runner sources every
# tests/linux/apps_e2e_<lane>.sh and calls `apps_e2e_<lane>`).
#
# What the runner provides (checked first; a missing one fails loudly):
#   IT, K2HOME, WS, DPORT, OWNER_TOKEN, HOSTIP   (as in apps_guarded_e2e.sh)
#   K2 asapp fail ok info jq_ wait_dep front mkapp   (its helper functions)
#
# Steps:
#   1. protocol 3 on a guarded unit: OOMPolicy=kill, no swap, a capped
#      k8s-file log read back through `k2 app logs`, the doctor's engine 3;
#   2. L1/Q4: an App with no memory cap has no --memory and no V8 heap flag;
#   3. K5: a NEW `k2 publish run --cmd` label becomes an open-path App in a
#      relaxed container: label, the floor from inside (caps 0, seccomp, no
#      user namespaces, no host loopback, no direct internet), the code a
#      writable copy (fresh at each start) and /data persistent, the old
#      --port inside, broad egress through the proxy with the host logged,
#      an extra port on host loopback, publish stop|start mapped onto it,
#      a re-run that deploys the folder's changes;
#   4. S16: a manifest build runs in the wall: install scripts off on the
#      guarded path, the registry reachable and nothing else, a package.json
#      without a lockfile refused.

apps_e2e_wall() {
	local v
	for v in IT K2HOME WS DPORT OWNER_TOKEN; do
		[ -n "${!v:-}" ] || { echo "FAIL apps_e2e_wall: the runner must set $v" >&2; exit 1; }
	done
	for v in K2 asapp fail ok info jq_ wait_dep front mkapp; do
		declare -F "$v" >/dev/null || { echo "FAIL apps_e2e_wall: the runner must define $v" >&2; exit 1; }
	done
	echo "== lane 1 (Wall): engine protocol 3 =="
	wall_owner() { # path json → body (fails on non-2xx)
		curl -sf -X POST -H 'Content-Type: application/json' -d "$2" "http://127.0.0.1:$DPORT$1?token=$OWNER_TOKEN"
	}
	wall_owner_any() { # path json → body, whatever the status
		curl -s -X POST -H 'Content-Type: application/json' -d "$2" "http://127.0.0.1:$DPORT$1?token=$OWNER_TOKEN"
	}
	curl -sf "http://127.0.0.1:$DPORT/cli/app/box?token=$OWNER_TOKEN" >/tmp/w-box.json || fail "GET /cli/app/box"
	jq_ 'st={c["id"]:c for c in v["checks"]}; e=st["engine"]; assert e["status"]=="ok", e; assert "protocol 3" in e["detail"], e' /tmp/w-box.json
	ok "the engine on this box speaks protocol 3"

	# ── 1. a guarded unit's resource lines (esc-a, capped at 256M) ─────
	local Q CTR
	Q="$(ls /var/lib/k2app/.config/containers/systemd/k2app-esc-a--*.container | head -n1)"
	[ -n "$Q" ] || fail "no esc-a Quadlet file"
	for want in "OOMPolicy=kill" "MemorySwapMax=0" "LogDriver=k8s-file" "--log-opt=max-size=52428800" "--memory=256m" "Ulimit=nofile=4096:4096" "Label=k2.wall=guarded"; do
		grep -q -- "$want" "$Q" || fail "esc-a unit lacks $want: $(cat "$Q")"
	done
	CTR="$(basename "$Q" .container)"
	asapp podman inspect "$CTR" >/tmp/w-ia.json
	jq_ 'c=v[0]; lc=c["HostConfig"]["LogConfig"]; assert lc["Type"]=="k8s-file", lc; assert str(lc.get("Size")) in ("52428800","50MiB","50MB","52.43MB"), lc' /tmp/w-ia.json
	K2 app logs esc-a >/tmp/w-la.txt 2>&1 || true
	grep -q "esc a2 up\|esc a1 up" /tmp/w-la.txt || fail "the App's output through the engine's logs verb: $(tail -c 1500 /tmp/w-la.txt)"
	ok "guarded unit: OOMPolicy=kill, no swap, 4096 files, a 50 MiB k8s-file log read back by k2 app logs"

	# ── 2. no memory cap (L1, Q4) ─────────────────────────────────────
	mkapp "$WS/n1" esc-n n1
	K2 app install esc-n
	K2 app deploy esc-n "$WS/n1" --engine guarded --no-wait >/dev/null || true
	local r FN
	r="$(wait_dep esc-n)"
	[ "$r" = live ] || { K2 app logs esc-n 2>&1 | tail -n 20; fail "esc-n (no cap) deploy: $r"; }
	FN="$(front esc-n)"
	curl -s "http://127.0.0.1:$FN/probe" >/tmp/w-pn.json
	jq_ 'n=v["nodeOptions"] or ""; assert "--max-old-space-size" not in n, n; assert "--report-on-fatalerror" in n, n' /tmp/w-pn.json
	CTR="$(asapp podman ps --format '{{.Names}}' | grep '^k2app-esc-n--' | head -n1)"
	asapp podman inspect "$CTR" >/tmp/w-in.json
	jq_ 'hc=v[0]["HostConfig"]; assert hc["Memory"]==0, hc["Memory"]' /tmp/w-in.json
	K2 app config esc-n --memory 512M >/dev/null
	K2 app restart esc-n --no-wait >/dev/null 2>&1 || K2 app restart esc-n >/dev/null
	r="$(wait_dep esc-n)"
	[ "$r" = live ] || fail "esc-n after a 512M cap: $r"
	FN="$(front esc-n)"
	curl -s "http://127.0.0.1:$FN/probe" | python3 -c 'import json,sys; n=json.load(sys.stdin)["nodeOptions"]; assert "--max-old-space-size=256" in n, n'
	K2 app uninstall esc-n --purge >/dev/null
	ok "no cap by default: no --memory, no V8 heap flag; a 512M cap gives the heap 256 (50%)"

	# ── 3. K5: a new --cmd label lands on the open path ───────────────
	echo "== lane 1: a new --cmd service is an open-path App (relaxed container) =="
	local SH="$WS/shop"
	mkapp "$SH" shop-unused shop
	rm -f "$SH/manifest.json"
	printf '9100' >"$SH/extra-port.txt"
	chown -R k2:k2 "$SH"
	K2 publish run shop --cmd "node server.mjs" --port 3000 --cwd "$SH" --no-tunnel --json >/tmp/w-run.json 2>/tmp/w-run.err \
		|| { cat /tmp/w-run.err; cat /tmp/w-run.json; fail "k2 publish run shop"; }
	jq_ 'w=v["wall"]; assert w["path"]=="open", w; assert w["label"]=="Open (container)", w; assert w["created"] is True, w' /tmp/w-run.json
	r="$(wait_dep shop)"
	[ "$r" = live ] || { K2 app deployment shop --json 2>&1 | tail -c 2000; K2 app logs shop 2>&1 | tail -n 30; fail "shop (open path) deploy: $r"; }
	local FS
	FS="$(front shop)"
	[ "$(curl -s "http://127.0.0.1:$FS/")" = shop ] || fail "front shop"
	K2 app list | grep -q "Open (container)" || fail "label Open (container) in k2 app list"
	K2 app inspect shop --json >/tmp/w-si.json
	jq_ 'a=v["app"]; assert a["path"]=="open" and a["source"]=="folder" and a["isolation"]=="container", a; assert "copied at each start" in (a.get("labelReason") or ""), a.get("labelReason"); assert a.get("labelBack")=="k2 app config shop --path guarded", a.get("labelBack")' /tmp/w-si.json
	ok "k2 publish run --cmd (new label): an open-path App, live behind a relaxed container, labelled Open (container) with the reason and the way back"
	Q="$(ls /var/lib/k2app/.config/containers/systemd/k2app-shop--*.container | head -n1)"
	for want in "Label=k2.wall=relaxed" ":/app:rw" "PublishPort=127.0.0.1:" ":3000" "DropCapability=all" "NoNewPrivileges=true" "SeccompProfile=" "localhost/k2-base/node24-full:"; do
		grep -q -- "$want" "$Q" || fail "shop unit lacks $want: $(cat "$Q")"
	done
	grep -q "ReadOnly=true" "$Q" && fail "a relaxed container with a read-only root"
	curl -s "http://127.0.0.1:$FS/probe" >/tmp/w-ps.json
	python3 - /tmp/w-ps.json <<'PY'
import json, sys
v = json.load(open(sys.argv[1]))
assert v["tag"] == "shop"
assert v["writeApp"]["ok"] is True, ("the code copy is writable", v["writeApp"])
assert v["writeUsr"]["ok"] is True, ("a writable root (the image tree)", v["writeUsr"])
assert v["writeData"]["ok"] is True and v["writeData"]["out"] == "shop", v["writeData"]
assert v["capEff"] == "0000000000000000" and v["capBnd"] == "0000000000000000", (v["capEff"], v["capBnd"])
assert v["noNewPrivs"] == "1" and v["seccomp"] == "2", (v["noNewPrivs"], v["seccomp"])
assert v["unshareU"]["ok"] is False, v["unshareU"]
assert v["daemonHome"] is False and v["ownerToken"] is False and v["k2appHome"] is False, v
for k in ("daemonPort", "hostSsh", "hostIpSsh", "internet"):
    assert v[k] != "open", (k, v[k])
assert v["gateway"]["status"] == 200, v["gateway"]
assert v["k2AppName"] == "shop", v["k2AppName"]
for bad in ("K2_HOOK_TOKEN", "K2_HOOK_SOCK", "K2_CELL", "K2_OWNER_TOKEN"):
    assert bad not in v["env"], bad
PY
	ok "inside the relaxed container: writable code copy and root, data rw, CapEff 0, NoNewPrivs, seccomp, no unshare -U, no daemon home/token, no host loopback or direct internet, its own gateway"

	K2 app egress shop --json >/tmp/w-eg.json
	jq_ 'assert v.get("mode")=="broad" or v.get("policy",{}).get("mode")=="broad", json.dumps(v)[:800]' /tmp/w-eg.json
	curl -s -m 40 "http://127.0.0.1:$FS/egress?target=example.com:443" >/tmp/w-ee.json
	jq_ 'assert v["connect"]=={"status":200}, v["connect"]; assert v["metadata"]["status"]==403, v["metadata"]; assert v["loopback"]["status"]==403, v["loopback"]' /tmp/w-ee.json
	K2 app egress shop --json >/tmp/w-eg2.json
	python3 -c 'import json,sys; v=json.load(open(sys.argv[1])); hosts=json.dumps(v); assert "example.com" in hosts, hosts[:1500]' /tmp/w-eg2.json
	ok "egress broad through the App's own proxy (K24): example.com through, metadata and loopback refused, the host logged"

	# Q3: the code is copied at each start; /data persists.
	[ "$(curl -s "http://127.0.0.1:$FS/stat?p=/app/pwn" | python3 -c 'import json,sys; print(json.load(sys.stdin)["exists"])')" = True ] || fail "the probe's write into /app"
	K2 app config shop --port add 9100 >/dev/null
	K2 publish stop shop --json >/tmp/w-stop.json || { cat /tmp/w-stop.json; fail "k2 publish stop shop (mapped)"; }
	jq_ 'assert v["wall"]["via"]=="/cli/app/stop", v' /tmp/w-stop.json
	K2 publish start shop --json >/tmp/w-start.json || { cat /tmp/w-start.json; fail "k2 publish start shop (mapped)"; }
	r="$(wait_dep shop)"
	[ "$r" = live ] || fail "shop after publish start: $r"
	FS="$(front shop)"
	[ "$(curl -s "http://127.0.0.1:$FS/stat?p=/app/pwn" | python3 -c 'import json,sys; print(json.load(sys.stdin)["exists"])')" = False ] || fail "/app is a fresh copy at each start"
	[ "$(curl -s "http://127.0.0.1:$FS/stat?p=/data/ok" | python3 -c 'import json,sys; print(json.load(sys.stdin)["text"])')" = shop ] || fail "/data persists across starts"
	[ "$(curl -s -m 5 "http://127.0.0.1:9100/")" = "extra shop" ] || fail "the extra port 9100 on host loopback"
	ok "k2 publish stop|start map onto the App; /app is a fresh copy at each start, /data persists; the extra port answers on host loopback"

	# A re-run deploys the folder's changes (a fresh snapshot).
	printf 'shop2' >"$SH/tag.txt"
	chown k2:k2 "$SH/tag.txt"
	K2 publish run shop --cmd "node server.mjs" --port 3000 --cwd "$SH" --no-tunnel --json >/tmp/w-run2.json 2>/tmp/w-run2.err \
		|| { cat /tmp/w-run2.err; fail "re-run shop"; }
	jq_ 'assert v["wall"]["created"] is False, v["wall"]' /tmp/w-run2.json
	r="$(wait_dep shop)"
	[ "$r" = live ] || fail "shop re-run: $r"
	FS="$(front shop)"
	[ "$(curl -s "http://127.0.0.1:$FS/")" = shop2 ] || fail "the re-run serves the new folder"
	[ "$(curl -s -m 5 "http://127.0.0.1:9100/")" = "extra shop2" ] || fail "the extra port after the re-run"
	ok "a re-run of the label deploys a fresh copy of the folder (with an extra port: the old instance stops first, then the new one holds 9100)"
	K2 publish rm shop --json >/tmp/w-rm.json || { cat /tmp/w-rm.json; fail "k2 publish rm shop (mapped)"; }
	K2 app list --json | python3 -c 'import json,sys; assert not [a for a in json.load(sys.stdin)["apps"] if a["appId"]=="shop"]'
	ok "k2 publish rm uninstalls the open-path App"

	# ── 4. S16: builds in the wall ────────────────────────────────────
	echo "== lane 1: manifest builds run inside the wall (S16) =="
	local B="$WS/bld"
	runuser -u k2 -- mkdir -p "$B"
	cat >"$B/manifest.json" <<'JSON'
{"schema":1,"kind":"app","id":"bld","name":"Build","version":"1.0.0",
 "build":["sh","-c","npm ci && echo built > dist.txt && (curl -s -m 8 -o /dev/null -w '%{http_code}' https://example.com/ > egress.txt || echo refused > egress.txt) && (curl -sS -m 20 -o /dev/null -w '%{http_code}' https://registry.npmjs.org/ > registry.txt 2>registry.err || echo refused > registry.txt)"],
 "host":{"mode":"cmd","start":["node","-e","require('http').createServer((q,s)=>s.end('bld')).listen(process.env.PORT)"]}}
JSON
	cat >"$B/package.json" <<'JSON'
{"name":"bld","version":"1.0.0","private":true,"scripts":{"postinstall":"node -e \"require('fs').writeFileSync('POSTINSTALL_RAN','x')\""}}
JSON
	cat >"$B/package-lock.json" <<'JSON'
{"name":"bld","version":"1.0.0","lockfileVersion":3,"requires":true,"packages":{"":{"name":"bld","version":"1.0.0","hasInstallScript":true}}}
JSON
	chown -R k2:k2 "$B"
	wall_owner /cli/app/build "{\"folder\":\"$B\"}" >/tmp/w-b.json || { wall_owner_any /cli/app/build "{\"folder\":\"$B\"}" | head -c 3000; fail "POST /cli/app/build"; }
	jq_ 'b=v["build"]; assert b["inWall"] is True, b' /tmp/w-b.json
	local SEED_HASH SD
	SEED_HASH="$(jq_ 'print(v["seed"]["hash"])' /tmp/w-b.json)"
	SD="$K2HOME/.k2/seeds/$SEED_HASH"
	[ -d "$SD" ] || fail "the built Seed $SEED_HASH is not in the store"
	[ "$(cat "$SD/dist.txt")" = built ] || fail "the build's output file"
	[ ! -e "$SD/POSTINSTALL_RAN" ] || fail "a postinstall script ran on the guarded path"
	[ "$(cat "$SD/egress.txt")" = refused ] || fail "the build reached a non-registry host: $(cat "$SD/egress.txt")"
	[ "$(cat "$SD/registry.txt")" = 200 ] || fail "the build couldn't reach the npm registry through its proxy: $(cat "$SD/registry.txt") $(cat "$SD/registry.err" 2>/dev/null) egress log: $(tail -n 5 "$K2HOME/.k2/apps/build/logs/egress.log" 2>/dev/null)"
	jq_ 'cw=v["seed"]["createdWith"]; assert cw["engine"]=="wall", cw; assert (v["seed"].get("builtFrom") or "").startswith("seed:"), v["seed"].get("builtFrom")' /tmp/w-b.json
	ok "k2 app build ran the manifest's build in the wall: install scripts off, the registry reachable, example.com refused, the tree stored as the Seed (createdWith.engine = wall)"
	rm -f "$B/package-lock.json"
	local code
	code="$(wall_owner_any /cli/app/build "{\"folder\":\"$B\"}" | python3 -c 'import json,sys; v=json.load(sys.stdin); print((v.get("error") or {}).get("code") or v.get("code") or "")')"
	[ "$code" = lockfile_required ] || fail "a package.json without a lockfile: $code"
	ok "a package.json without a lockfile is refused (lockfile_required)"
	echo "  lane 1 (Wall) phase: done"
}
