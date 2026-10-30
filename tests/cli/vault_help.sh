#!/usr/bin/env bash
# `k2 vault` help is enough to tell "possible" from "not finding it" (0.46.4,
# from an ops agent's six questions):
#   * every `k2 vault <verb> --help` prints that verb's own help (not the
#     whole top-level page), with no daemon at all;
#   * `set --help` lists the inject forms, each --kind's delivery and the
#     --sign options; `grant --help` shows --env-name with a CLI-tool
#     example; `fetch --help` says some secrets are signers and documents
#     --json-out;
#   * `k2 study vault` has the start-here, kinds, CLI-tools, systemd-creds
#     and can't-do-yet sections and stays bounded;
#   * against a stub daemon: `list --system` prints the hidden count and
#     why, `doctor` prints the names it checks, and `set --inject-dst` sends
#     that dst (query/basic/body; --inject-header still works, both refused);
#     `set --sign …` sends inject.sign and `k2 vault sign NAME …|none` posts
#     the signer to policy (PRD vault-gaps item 2);
#   * item 3 (agent delivery): set/delivery/grant help name agent, the
#     agent-limit help carries the limits and the honest line, `k2 study
#     vault` has the agent row and no "ssh-agent" can't-do line, and
#     `agent-limit` / `public-key` call their routes.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
K2_CLI="$(cd "$SCRIPT_DIR/../.." && pwd)/cli/k2"
pass=0
fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "  ok $*"; pass=$((pass + 1)); }
for v in $(env | grep -E '^K2(SO)?_' | cut -d= -f1); do unset "$v"; done

WORK="$(mktemp -d -t k2-vault-help-XXXXXX)"
STUB_PID=""
trap '[ -n "$STUB_PID" ] && { kill "$STUB_PID" 2>/dev/null; wait "$STUB_PID" 2>/dev/null || true; }; rm -rf "$WORK"' EXIT

# ── help with no daemon: no port, no token, empty HOME ──────────────────
k2n() { env HOME="$WORK" "$K2_CLI" "$@"; }

top="$(k2n vault --help)" || fail "vault --help"
printf '%s\n' "$top" | head -1 | grep -Fq "k2 vault — secrets for agents" || fail "top-level help: $top"
printf '%s' "$top" | grep -Fq "k2 vault <verb> --help" || fail "top-level help doesn't point at per-verb help"
printf '%s' "$top" | grep -Fq -- "--inject-dst header:<Name>|query:<param>|basic|body" || fail "top-level help lacks --inject-dst"
ok "k2 vault --help: no daemon needed, names --inject-dst and per-verb help"

for verb in status list show set rotate hosts delivery grant revoke disable enable destroy get audit requests presets \
    doctor env-file import service reveal recovery confirm-password kek rekey rollback backup restore recipient carry fetch send sign \
    systemd-cred placement placements public-key agent-limit; do
    out="$(k2n vault "$verb" --help 2>&1)" || fail "vault $verb --help exited non-zero: $out"
    case "$verb" in
        enable) want="k2 vault disable NAME" ;;
        rekey) want="k2 vault kek rotate" ;;
        send) want="k2 vault send" ;;
        placements) want="k2 vault placement add" ;;
        *) want="k2 vault $verb" ;;
    esac
    printf '%s\n' "$out" | head -1 | grep -Fq "$want" || fail "vault $verb --help starts with: $(printf '%s\n' "$out" | head -1)"
    if printf '%s' "$out" | grep -Fq "k2 vault — secrets for agents"; then fail "vault $verb --help printed the top-level help"; fi
    printf '%s' "$out" | grep -Eq "^Example|^  k2 vault|^  new box:|^Examples|^  k2 vault send" || fail "vault $verb --help has no example: $out"
done
ok "every k2 vault verb has its own --help with an example"

# K2 Cores field report (2026-10-28): `k2 publish secret --help` is its own
# page with the flags and an example (it used to be a usage error).
for args in "publish secret --help" "publish secret add --help" "publish secret help" "publish secret add mpc NAME -h"; do
    # shellcheck disable=SC2086
    out="$(k2n $args 2>&1)" || fail "$args exited non-zero: $out"
    printf '%s\n' "$out" | head -1 | grep -Fq "k2 publish secret — vault secrets a published service gets" || fail "$args: $out"
    for want in "--scope <s>" "--as env|file" "--env-name X" "--file-name F" \
        "k2 publish secret add mpc-progress MPC_PROGRESS_PIN --as env" "with --scope ws:k2-cores added" \
        "k2 permission request vault.share-service as=file|env secret=NAME service=<service>" "k2 app restart <app>"; do
        printf '%s' "$out" | grep -Fq -- "$want" || fail "$args lacks '$want': $out"
    done
done
set +e
out="$(k2n publish secret 2>&1)"; rc=$?
set -e
[ "$rc" = 2 ] && printf '%s' "$out" | grep -Fq -- "--env-name X" || fail "bare publish secret: exit $rc ($out)"
ok "k2 publish secret --help lists --scope, --as, --env-name, --file-name with an example"

out="$(k2n vault set NAME --kind ssh_key --help)" || fail "set --help after flags"
printf '%s\n' "$out" | head -1 | grep -Fq "k2 vault set NAME [options] < value" || fail "help after flags: $out"
ok "--help anywhere in the args prints the verb's help"

set_help="$(k2n vault set --help)"
for needle in "--inject-header <Name>" "--inject-format '<fmt>'" "--inject-dst <d>" "header:<Name>" "query:<param>" \
    "basic " "body " "{{secret}}" "api_key, oauth_token, generic   fetch" "password, dsn, url              env" \
    "ssh_key, tls_key                file" "Every kind allows fetch, file and env" "--sign ovh" "--sign hmac-sha256 " \
    "--sign hmac-sha256-ts" "--sign-header <Name>" "--sign-ts-header <Name>" "--sign-template '<t>'" "--sign-value '<v>'" \
    "ovh-eu|ovh-ca|ovh-us" "--delivery fetch|agent|file|env|api" "ssh_key also allows agent" "--kind ssh_key --delivery agent"; do
    printf '%s' "$set_help" | grep -Fq -- "$needle" || fail "set --help lacks '$needle'"
done
ok "set --help: inject options, the four dst forms, delivery per kind"

# prd-vault-gaps-v1 item 3: agent delivery.
printf '%s' "$(k2n vault delivery --help)" | grep -Fq 'agent (ssh keys): $SSH_AUTH_SOCK' || fail "delivery --help lacks agent"
printf '%s' "$(k2n vault grant --help)" | grep -Fq -- "--as fetch|agent|file|env|api" || fail "grant --help lacks --as agent"
al_help="$(k2n vault agent-limit --help)"
for needle in "--burst N" "--per-minute M" "--allow-forwarded" "default 60" "default 240" "rate_limited" \
    "Advisory, not a security control" "can still sign with it" "come from what the ssh client" "can't be told apart."; do
    printf '%s' "$al_help" | grep -Fq -- "$needle" || fail "agent-limit --help lacks '$needle'"
done
printf '%s' "$(k2n vault public-key --help)" | grep -Fq "Never the" || fail "public-key --help"
set +e
err="$(env HOME="$WORK" K2_PORT=1 K2_HOOK_TOKEN=offline "$K2_CLI" vault agent-limit X --allow-forwarded --no-forwarded 2>&1)"; rc=$?
set -e
[ "$rc" = 2 ] || fail "agent-limit with both forwarding flags: exit $rc ($err)"
set +e
err="$(env HOME="$WORK" K2_PORT=1 K2_HOOK_TOKEN=offline "$K2_CLI" vault agent-limit X --burst ten 2>&1)"; rc=$?
set -e
[ "$rc" = 2 ] || fail "agent-limit --burst ten: exit $rc ($err)"
ok "agent delivery: set/delivery/grant/agent-limit/public-key help, bad flags exit 2"

grant_help="$(k2n vault grant --help)"
printf '%s' "$grant_help" | grep -Fq -- "--env-name X" || fail "grant --help lacks --env-name"
printf '%s' "$grant_help" | grep -Fq "k2 vault grant GITHUB_CLI --to ws:<you> --env-name GH_TOKEN" || fail "grant --help lacks the CLI-tool example"
ok "grant --help: --env-name with a CLI-tool example"

fetch_help="$(k2n vault fetch --help)"
printf '%s' "$fetch_help" | grep -Fq "fetch adds the secret as-is, in one place: a header, a query parameter," || fail "fetch --help lacks the one-place line"
printf '%s' "$fetch_help" | grep -Fq "Some secrets are signers: K2 signs the request (OVH, HMAC, HMAC with a" || fail "fetch --help lacks the signer line"
if printf '%s' "$fetch_help" | grep -Fq "It can't sign"; then fail "fetch --help still says it can't sign"; fi
printf '%s' "$fetch_help" | grep -Fq -- "--json-out prints the daemon's answer" || fail "fetch --help lacks the --json-out shape"
printf '%s' "$fetch_help" | grep -Fq '"bodyB64"' || fail "fetch --help lacks bodyB64"
ok "fetch --help: as-is in one place, signers, --json-out shape"

# prd-vault-gaps-v1 item 4: systemd-cred and placements.
sc_help="$(k2n vault systemd-cred --help)"
for needle in "People only, on Linux" "LoadCredentialEncrypted=<cred>:/etc/k2/credstore.encrypted/<cred>.cred" "--replace" "/etc/k2-root-helper/systemd-cred-units" \
    "\$CREDENTIALS_DIRECTORY/<cred>" "--restart" "--remove" "--dry-run" "%d/k2c.env" "ExecStart=" "single quotes"; do
    printf '%s' "$sc_help" | grep -Fq -- "$needle" || fail "systemd-cred --help lacks '$needle'"
done
pl_help="$(k2n vault placement --help)"
for needle in "k2 vault placement add NAME --where <host>:<path>" "k2 vault placement done ID" "k2 vault placements [NAME]"; do
    printf '%s' "$pl_help" | grep -Fq -- "$needle" || fail "placement --help lacks '$needle'"
done
set +e
err="$(env HOME="$WORK" K2_PORT=1 K2_HOOK_TOKEN=offline "$K2_CLI" vault systemd-cred X 2>&1)"; rc=$?
set -e
[ "$rc" = 2 ] || fail "systemd-cred without --unit: exit $rc ($err)"
set +e
err="$(env HOME="$WORK" K2_PORT=1 K2_HOOK_TOKEN=agent-token "$K2_CLI" vault systemd-cred X --unit a.service 2>&1)"; rc=$?
set -e
if [ "$(uname -s)" = "Linux" ]; then
    [ "$rc" = 3 ] || fail "systemd-cred from a K2 terminal: exit $rc ($err)"
    printf '%s' "$err" | grep -Fq "owner_only" || fail "systemd-cred from a K2 terminal: $err"
else
    [ "$rc" = 2 ] || fail "systemd-cred on $(uname -s): exit $rc ($err)"
    printf '%s' "$err" | grep -Fq "linux_only" || fail "systemd-cred on $(uname -s): $err"
fi
ok "systemd-cred and placement help; people only; Linux only"

# ── k2 study vault ────────────────────────────────────────────────────
study="$(env HOME="$WORK" K2_PORT=1 K2_HOOK_TOKEN=offline "$K2_CLI" study vault)" || fail "study vault"
for section in "START HERE" "KINDS AND DELIVERY" "CLI TOOLS" "SERVICES K2 DOESN'T RUN" "NO K2 ON THE BOX" "WHAT THE VAULT CAN'T DO YET"; do
    printf '%s\n' "$study" | grep -q "^$section" || fail "study vault lacks $section"
done
for needle in "k2 vault import .env" "k2 vault send NAME --to <agent>" "ssh -i \$K2_SECRETS_DIR/NAME" "-o IdentitiesOnly=yes" \
    "--env-name GH_TOKEN" "hosts.yml" "auth.json" "systemd-creds encrypt" "LoadCredentialEncrypted=" "\$CREDENTIALS_DIRECTORY/" \
    "send and carry need K2 on both ends" "sign requests (AWS SigV4, Google service-account JWT)" "\$SSH_AUTH_SOCK" \
    "--kind ssh_key --delivery agent" "k2 vault public-key NAME" "The ssh -A refusal and audit host key are" "advisory: they trust what the ssh client says" \
    "Some secrets are signers" "--sign ovh --preset ovh-eu" "--json-out" \
    "k2 vault systemd-cred API_KEY --unit" "k2 vault placement add" "Keep the source copy in a K2 vault" "%d/<cred>"; do
    printf '%s' "$study" | grep -Fq -- "$needle" || fail "study vault lacks '$needle'"
done
# The can't-do-yet block is the last section.
last="$(printf '%s\n' "$study" | grep -E '^[A-Z][A-Z ,()'"'"'-]+$|^[A-Z][A-Z '"'"']+ \(' | tail -1)"
case "$last" in "WHAT THE VAULT CAN'T DO YET"*) ;; *) fail "the can't-do-yet block isn't last (last section: $last)" ;; esac
lines="$(printf '%s\n' "$study" | wc -l | tr -d ' ')"
[ "$lines" -le 180 ] || fail "study vault is $lines lines (max 180)"
if printf '%s' "$study" | grep -Fq "plain systemd units"; then fail "study vault still lists plain systemd units as can't-do"; fi
if printf '%s' "$study" | grep -Fq "ssh-agent: file delivery"; then fail "study vault still lists ssh-agent as can't-do"; fi
ok "k2 study vault: new sections present, can't-do-yet last, $lines lines"

# ── against a stub daemon ─────────────────────────────────────────────
PORT="$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"
cat >"$WORK/stub.py" <<'PY2'
import http.server, json, sys
LOG = sys.argv[2]
WHY = "K2's own secrets are used by the daemon only"
CHECKED = "checked names: *SECRET*, *TOKEN*, *PASSWORD*, *_KEY, DATABASE_URL, *_DSN (not NEXT_PUBLIC_*, *_PUBLISHABLE_KEY, *_ANON_KEY). Other names aren't flagged; check them yourself."
class H(http.server.BaseHTTPRequestHandler):
    def reply(self, out):
        data = json.dumps(out).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def do_GET(self):
        if self.path.startswith("/cli/vault/list"):
            out = {"secrets": []}
            if "system=1" in self.path:
                out["systemHidden"] = {"count": 44, "why": WHY}
            self.reply(out)
        elif self.path.startswith("/cli/vault/doctor"):
            self.reply({"ok": True, "findings": [], "checkedNames": CHECKED})
        elif self.path.startswith("/cli/vault/public-key"):
            with open(LOG, "a") as f:
                f.write("GET " + self.path + "\n")
            self.reply({"name": "DEPLOY_KEY", "publicKey": "ssh-ed25519 AAAAk2test DEPLOY_KEY", "fingerprint": "SHA256:k2test"})
        else:
            self.reply({})
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(n).decode()
        with open(LOG, "a") as f:
            f.write(self.path.split("?")[0] + " " + body + "\n")
        if self.path.startswith("/cli/vault/grant"):
            b = json.loads(body or "{}")
            if b.get("name") == "REFUSED_CARD":
                data = json.dumps({"error": {"code": "vault_owner_confirm_required", "hint": "a person gave this secret. The owner or an Admin can allow it on a permission card: run the line below to ask them.", "secret": "REFUSED_CARD", "service": "svc", "as": "file"},
                                   "permission": {"action": "vault.share-service", "params": {"as": "file", "secret": "REFUSED_CARD", "service": "svc"}, "approvers": "owner or admin", "command": "k2 permission request vault.share-service as=file secret=REFUSED_CARD service=svc --why \"<what you need it for>\""}}).encode()
                self.send_response(403)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)
                return
            if b.get("name") in ("REFUSED_ON", "REFUSED_OFF"):
                err = {"error": {"code": "vault_owner_confirm_required", "hint": "a person gave this secret, so an agent can't widen where it goes or who can read it. Settings → Vault can't share with a service, so ask the owner to run this in their own terminal outside K2 (it asks for Touch ID or the vault password): k2 publish secret add svc " + b["name"] + " --scope ws:w --as env"}}
                if b["name"] == "REFUSED_ON":
                    err["permission"] = {"requestable": False, "where": "a person does this, in Settings or from a terminal outside K2"}
                data = json.dumps(err).encode()
                self.send_response(403)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)
                return
        if self.path.startswith("/cli/vault/file"):
            b = json.loads(body or "{}")
            if b.get("done"):
                return self.reply({"ok": True, "removed": b["done"]})
            return self.reply({"ok": True, "path": "/run/k2-secrets/use-files/abc-" + b["secret"], "name": b["secret"], "ttlSecs": b.get("ttlSecs", 60)})
        self.reply({"ok": True, "created": True, "ref": "k2v://ws/w/X", "secret": {"name": "X", "latestVersion": 1}, "use": "k2 vault fetch --secret X https://h/…"})
    def log_message(self, *a):
        pass
http.server.HTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
PY2
python3 "$WORK/stub.py" "$PORT" "$WORK/posts.log" &
STUB_PID=$!
for _ in $(seq 1 100); do curl -s "http://127.0.0.1:$PORT/" >/dev/null 2>&1 && break; sleep 0.05; done
k2s() { env HOME="$WORK" K2_HOST=127.0.0.1 K2_PORT="$PORT" K2_HOOK_TOKEN=stub-token K2_PROJECT_PATH="$WORK" "$K2_CLI" "$@"; }

out="$(k2s vault list --system)" || fail "list --system: $out"
printf '%s' "$out" | grep -Fq "44 K2 system secrets hidden from you: K2's own secrets are used by the daemon only" || fail "list --system: $out"
if printf '%s' "$out" | grep -Fq "no secrets yet"; then fail "list --system says 'no secrets yet' next to hidden ones: $out"; fi
out="$(k2s vault list)" || fail "list: $out"
if printf '%s' "$out" | grep -Fq "hidden"; then fail "plain list mentions hidden system secrets: $out"; fi
ok "list --system prints the hidden count and why; plain list doesn't"

out="$(k2s vault doctor)" || fail "doctor: $out"
printf '%s\n' "$out" | tail -1 | grep -Fq ".env checked names: *SECRET*, *TOKEN*, *PASSWORD*, *_KEY, DATABASE_URL, *_DSN (not NEXT_PUBLIC_*" || fail "doctor: $out"
ok "doctor prints the names it checks"

: >"$WORK/posts.log"
printf 'k2test-value' | k2s vault set W --for api.example.com --inject-dst query:appid >/dev/null || fail "set --inject-dst query"
printf 'k2test-value' | k2s vault set M --for api.example.com --inject-dst basic --inject-format 'api:{}' >/dev/null || fail "set --inject-dst basic"
printf 'k2test-value' | k2s vault set H --for api.example.com --inject-header X-Api-Key >/dev/null || fail "set --inject-header"
python3 - "$WORK/posts.log" <<'PY2' || fail "set bodies: $(cat "$WORK/posts.log")"
import json, sys
rows = [l.split(" ", 1) for l in open(sys.argv[1]).read().splitlines()]
got = {json.loads(b)["name"]: json.loads(b)["inject"] for p, b in rows if p == "/cli/vault/set"}
assert got["W"] == {"dst": "query:appid", "fmt": "{}"}, got
assert got["M"] == {"dst": "basic", "fmt": "api:{}"}, got
assert got["H"] == {"dst": "header:X-Api-Key", "fmt": "{}"}, got
PY2
set +e
err="$(printf 'k2test-value' | k2s vault set B --inject-header X --inject-dst query:q 2>&1)"
rc=$?
set -e
[ "$rc" = 2 ] || fail "--inject-header with --inject-dst: exit $rc ($err)"
printf '%s' "$err" | grep -Fq "give one of them" || fail "--inject-header with --inject-dst: $err"
ok "set --inject-dst sends query/basic dst; --inject-header still works; both at once is refused"

: >"$WORK/posts.log"
printf '{}' | k2s vault set OVH_EU --sign ovh --preset ovh-eu >/dev/null || fail "set --sign ovh"
printf 'k2test-value' | k2s vault set HOOK --for hooks.example.com --sign hmac-sha256 --sign-header X-Hub-Signature-256 --sign-prefix 'sha256=' >/dev/null || fail "set --sign hmac-sha256"
printf 'k2test-value' | k2s vault set OPS --for ingest.example.com --sign hmac-sha256-ts --sign-header X-K2-Ops-Signature \
    --sign-ts-header X-K2-Ops-Ts --sign-value 'sha256={sig}' --sign-encoding base64 >/dev/null || fail "set --sign hmac-sha256-ts"
k2s vault sign HOOK none >/dev/null || fail "sign none"
k2s vault sign HOOK hmac-sha256 --sign-header X-Sig --sign-prefix '' >/dev/null || fail "sign hmac-sha256"
python3 - "$WORK/posts.log" <<'PY2' || fail "signer bodies: $(cat "$WORK/posts.log")"
import json, sys
rows = [(p, json.loads(b)) for p, b in (l.split(" ", 1) for l in open(sys.argv[1]).read().splitlines())]
sets = {b["name"]: b for p, b in rows if p == "/cli/vault/set"}
assert sets["OVH_EU"]["inject"] == {"sign": {"scheme": "ovh"}} and sets["OVH_EU"]["preset"] == "ovh-eu", sets["OVH_EU"]
assert sets["HOOK"]["inject"] == {"sign": {"scheme": "hmac-sha256", "header": "X-Hub-Signature-256", "prefix": "sha256="}}, sets["HOOK"]
assert sets["OPS"]["inject"] == {"sign": {"scheme": "hmac-sha256-ts", "header": "X-K2-Ops-Signature", "encoding": "base64",
                                          "tsHeader": "X-K2-Ops-Ts", "valueFmt": "sha256={sig}"}}, sets["OPS"]
pol = [b for p, b in rows if p == "/cli/vault/policy"]
assert pol[0]["name"] == "HOOK" and pol[0]["sign"] == "none", pol
assert pol[1]["sign"] == {"scheme": "hmac-sha256", "header": "X-Sig", "prefix": ""}, pol
PY2
set +e
err="$(printf 'k2test-value' | k2s vault set B --sign ovh --inject-header X 2>&1)"; rc1=$?
err2="$(printf 'k2test-value' | k2s vault set B --sign-header X 2>&1)"; rc2=$?
err3="$(k2s vault sign HOOK none --sign-header X 2>&1)"; rc3=$?
set -e
[ "$rc1" = 2 ] && printf '%s' "$err" | grep -Fq "no --inject-*" || fail "--sign with --inject-*: exit $rc1 ($err)"
[ "$rc2" = 2 ] && printf '%s' "$err2" | grep -Fq "need --sign" || fail "--sign-header alone: exit $rc2 ($err2)"
[ "$rc3" = 2 ] || fail "sign none with options: exit $rc3 ($err3)"
ok "set --sign sends inject.sign; k2 vault sign posts the signer or none; mixed flags refused"

# Item 3: agent-limit posts the settings; public-key prints the .pub line.
: >"$WORK/posts.log"
k2s vault agent-limit DEPLOY_KEY --burst 10 --per-minute 60 --no-forwarded >/dev/null || fail "agent-limit"
k2s vault agent-limit DEPLOY_KEY --allow-forwarded >/dev/null || fail "agent-limit --allow-forwarded"
printf 'k2test-value' | k2s vault set DEPLOY_KEY --kind ssh_key --delivery agent >/dev/null || fail "set --delivery agent"
out="$(k2s vault public-key DEPLOY_KEY 2>/dev/null)" || fail "public-key"
[ "$out" = "ssh-ed25519 AAAAk2test DEPLOY_KEY" ] || fail "public-key printed: $out"
python3 - "$WORK/posts.log" <<'PY2' || fail "agent bodies: $(cat "$WORK/posts.log")"
import json, sys
lines = open(sys.argv[1]).read().splitlines()
posts = [(p, json.loads(b)) for p, b in (l.split(" ", 1) for l in lines if not l.startswith("GET "))]
ap = [b for p, b in posts if p == "/cli/vault/agent-policy"]
assert ap[0] == {"name": "DEPLOY_KEY", "burst": 10, "perMinute": 60, "allowForwarded": False}, ap
assert ap[1] == {"name": "DEPLOY_KEY", "allowForwarded": True}, ap
st = [b for p, b in posts if p == "/cli/vault/set"]
assert st[0]["kind"] == "ssh_key" and st[0]["delivery"] == "agent", st
import urllib.parse
gets = [urllib.parse.urlsplit(l[4:]) for l in lines if l.startswith("GET ")]
assert [g.path for g in gets] == ["/cli/vault/public-key"], gets
q = urllib.parse.parse_qs(gets[0].query)
assert q.get("name") == ["DEPLOY_KEY"], q
PY2
ok "agent-limit posts burst/perMinute/allowForwarded; set --delivery agent; public-key prints the line"

# 2026-10-28: k2 vault file posts one secret (name only) and prints the path.
: >"$WORK/posts.log"
out="$(k2s vault file --secret ENROLL_CODE --ttl 120)" || fail "file: $out"
[ "$out" = "/run/k2-secrets/use-files/abc-ENROLL_CODE" ] || fail "file printed: $out"
out="$(k2s vault file ENROLL_CODE)" || fail "file NAME: $out"
out="$(k2s vault file --done /run/k2-secrets/use-files/abc-ENROLL_CODE)" || fail "file --done: $out"
[ "$out" = "removed /run/k2-secrets/use-files/abc-ENROLL_CODE" ] || fail "file --done printed: $out"
python3 - "$WORK/posts.log" <<'PY2' || fail "file bodies: $(cat "$WORK/posts.log")"
import json, sys
rows = [(p, json.loads(b)) for p, b in (l.split(" ", 1) for l in open(sys.argv[1]).read().splitlines())]
assert [p for p, _ in rows] == ["/cli/vault/file"] * 3, rows
assert rows[0][1] == {"secret": "ENROLL_CODE", "ttlSecs": 120}, rows
assert rows[1][1] == {"secret": "ENROLL_CODE"}, rows
assert rows[2][1] == {"done": "/run/k2-secrets/use-files/abc-ENROLL_CODE"}, rows
PY2
set +e
err="$(k2s vault file --secret A --secret B 2>&1)"; rc1=$?
err2="$(k2s vault file --secret A --ttl soon 2>&1)"; rc2=$?
err3="$(k2s vault file 2>&1)"; rc3=$?
set -e
[ "$rc1" = 2 ] && printf '%s' "$err" | grep -Fq "one secret per file" || fail "two secrets: exit $rc1 ($err)"
[ "$rc2" = 2 ] || fail "--ttl soon: exit $rc2 ($err2)"
[ "$rc3" = 2 ] || fail "no secret: exit $rc3 ($err3)"
ok "file posts one secret by name (--ttl, --done) and prints only the path"

# K2 Cores field report (2026-10-28): a vault refusal ends with the
# permission closing line when the daemon sends one (cards on), and with
# nothing extra when it doesn't (cards off: today's text).
set +e
err_on="$(k2s publish secret add svc REFUSED_ON --as env 2>&1)"; rc_on=$?
err_off="$(k2s publish secret add svc REFUSED_OFF --as env 2>&1)"; rc_off=$?
set -e
[ "$rc_on" = 3 ] && [ "$rc_off" = 3 ] || fail "refused grant exits 3 (on $rc_on, off $rc_off): $err_on / $err_off"
[ "$(printf '%s\n' "$err_on" | tail -1)" = "It can't be requested." ] || fail "cards on: no closing line: $err_on"
[ "$(printf '%s\n' "$err_on" | grep -c "It can't be requested.")" = 1 ] || fail "cards on: closing line printed more than once: $err_on"
printf '%s\n' "$err_on" | head -1 | grep -Fq "vault_owner_confirm_required: a person gave this secret" || fail "cards on: hint first: $err_on"
[ "$(printf '%s\n' "$err_off" | wc -l | tr -d ' ')" = 1 ] || fail "cards off: one line, as before: $err_off"
printf '%s' "$err_off" | grep -Fq "k2 publish secret add svc REFUSED_OFF --scope ws:w --as env" || fail "cards off: hint: $err_off"
grep -F '/cli/vault/grant {' "$WORK/posts.log" | grep -F '"REFUSED_ON"' | grep -Fq '"to": "service:svc"' || fail "publish secret add posts a service grant: $(cat "$WORK/posts.log")"
ok "a refused vault grant ends with It can't be requested. (cards on) and stays one line (cards off)"

# A share a card can do: the closing line is the exact one to run.
set +e
err_card="$(k2s publish secret add svc REFUSED_CARD --as file 2>&1)"; rc_card=$?
set -e
[ "$rc_card" = 3 ] || fail "refused share exits 3: $rc_card: $err_card"
[ "$(printf '%s\n' "$err_card" | tail -1)" = 'Ask with: k2 permission request vault.share-service as=file secret=REFUSED_CARD service=svc --why "<what you need it for>"' ] || fail "no Ask-with line: $err_card"
[ "$(printf '%s\n' "$err_card" | grep -c '^Ask with:')" = 1 ] || fail "the Ask-with line printed more than once: $err_card"
ok "a refused share a card can do ends with the Ask-with line"

# --cwd relative to where k2 runs, not the daemon's folder.
mkdir -p "$WORK/site"
(cd "$WORK" && k2s publish run demo --cmd 'true' --port 4321 --no-tunnel --cwd site >/dev/null 2>&1) || true
want_cwd="$(cd "$WORK/site" && pwd -P)"
grep -F '/cli/publish/run' "$WORK/posts.log" | grep -Fq "\"cwd\": \"$want_cwd\"" || fail "--cwd site → $want_cwd: $(grep -F '/cli/publish/run' "$WORK/posts.log")"
ok "publish run --cwd site resolves against the caller's folder"

echo "PASS: vault help ($pass checks)"
