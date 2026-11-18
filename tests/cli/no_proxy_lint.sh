#!/usr/bin/env bash
# Lint: the CLI never sends a request to the daemon through a proxy, and
# never puts a credential in a request's query string.
#
# The rule: loopback (127.0.0.1, ::1, localhost) and plain http never use
# a proxy; HTTPS to a remote K2 may use the person's proxy (a CONNECT
# tunnel hides the request; a company proxy that intercepts TLS is the
# organisation's own policy). This lint checks the loopback half by text;
# tests/cli/no_proxy_to_daemon.sh and the app's proxy test check both.
#
# Reads cli/src (the parts cli/k2 is built from) and fails, naming file and
# line, when:
#   1. _k2_curl no longer adds --noproxy '*' for daemon calls;
#   2. a curl runs outside _k2_curl, in a file not on the outside-host list
#      below (a daemon call must go through _k2_curl);
#   3. a Python block calls urlopen without installing an opener with an
#      empty ProxyHandler first, in a file not on the outside-host list;
#   4. a request line builds a token query parameter (?token=, &token=,
#      "token=$…", data-urlencode "token=…", dict(…, token=token),
#      {"token": tok…}, q["token"] = …). The credential rides Authorization: Bearer.
#
# Outside hosts keep the person's proxy settings. They are named here, with
# what they fetch, so a new one is a reviewed change:
#   update.sh          GitHub releases API and the app download
#   daemon-install.sh  the daemon manifest, binary and signature downloads
#   connect-account.sh the K2 Connect account service (sign-in, logout)
#
# It also reads the scripts outside cli/src that call the daemon with curl
# (NAMED below) and fails on a curl command line there without
# --noproxy '*', except the outside-host lines named with them.
#
# And it reads the Rust sources (src-tauri/src, crates/*/src): an HTTP
# client built without .no_proxy() or a token query parameter fails unless
# its file is on the reviewed lists below (RUST_OUTSIDE, RUST_TOKEN_OK).
#
# Before the real scan it runs itself on a scratch dir of broken and good
# parts and fails unless it catches exactly the broken ones.
#
# Run with: bash tests/cli/no_proxy_lint.sh [dir]   (default cli/src)
# bash-only: reads the bash CLI's parts as text.
set -euo pipefail
export LC_ALL=C

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

lint() { # lint <dir>: prints one line per problem; exit 1 when any
    python3 - "$1" <<'PYEOF'
import glob, os, re, sys

src = sys.argv[1]
OUTSIDE = {"update.sh", "daemon-install.sh", "connect-account.sh"}
TOKEN_Q = [
    re.compile(r"[?&]token="),
    re.compile(r'data-urlencode\s+"token='),
    re.compile(r"[\"']token=\$"),
    re.compile(r"dict\([^)]*\btoken=token\b"),
    re.compile(r'\{\s*"token"\s*:\s*(tok|token)\b'),
    re.compile(r'\[\s*"token"\s*\]\s*='),
]
HEREDOC = re.compile(r"<<(-?)\s*['\"]?([A-Za-z_][A-Za-z0-9_]*)['\"]?")
REQUEST_LINE = re.compile(r"_k2_cfg_add|\bcurl\b|urlopen|Request\(|\burl\s*=|\bqs\s*=|\bparams\s*=|\bq\s*=|\bpath=")
problems = []
files = sorted(glob.glob(os.path.join(src, "*.sh")))
if not files:
    print("no parts found under %s" % src)
    sys.exit(2)

curl_fn_ok = False
for f in files:
    name = os.path.basename(f)
    lines = open(f, encoding="utf-8", errors="replace").read().split("\n")
    in_k2_curl = False
    py_start = None  # line index of the nearest python3 launch
    heredoc = None   # (delimiter, strip_tabs, is_python) while inside one
    for i, line in enumerate(lines):
        s = line.strip()
        if heredoc is not None:
            delim, strip_tabs, is_py = heredoc
            if (line.lstrip("\t") if strip_tabs else line) == delim:
                heredoc = None
                continue
            if not is_py:
                continue  # help text and other data: not run as a request
        else:
            m = HEREDOC.search(line)
            if m and not s.startswith("#"):
                heredoc = (m.group(2), m.group(1) == "-", bool(re.search(r"\bpython3\b", line)))
        if line.startswith("_k2_curl() {"):
            in_k2_curl = True
        elif in_k2_curl and line.startswith("}"):
            in_k2_curl = False
        if in_k2_curl and "set -- --noproxy '*' \"$@\"" in line:
            curl_fn_ok = True
        if re.search(r"\bpython3\b", line):
            py_start = i
        if s.startswith("#"):
            continue
        # 2. a curl outside _k2_curl
        if heredoc is None and not in_k2_curl and name not in OUTSIDE and re.search(r"(^|[\s;|&(`$])curl\s+-", line) \
                and "_k2_curl" not in line and "command -v curl" not in line:
            problems.append("%s:%d: curl outside _k2_curl: %s" % (name, i + 1, s[:100]))
        # 3. urlopen with no proxy-free opener in its block
        if "urlopen(" in line and name not in OUTSIDE:
            block = "\n".join(lines[(py_start if py_start is not None else 0):i])
            if "install_opener(urllib.request.build_opener(urllib.request.ProxyHandler({})))" not in block:
                problems.append("%s:%d: urlopen without an empty ProxyHandler opener" % (name, i + 1))
        # 4. a token in the query
        if name not in OUTSIDE and REQUEST_LINE.search(line) and any(p.search(line) for p in TOKEN_Q):
            problems.append("%s:%d: a credential in the query string: %s" % (name, i + 1, s[:100]))

if not curl_fn_ok:
    problems.append("_k2_curl does not add --noproxy '*' for daemon calls")
for p in problems:
    print(p)
sys.exit(1 if problems else 0)
PYEOF
}

# Scripts outside cli/src that reach the daemon with curl, as
# "<path>|<outside-host line marker>|...". A marker is text on a curl line
# that fetches an outside host and keeps the proxy settings.
NAMED=(
    "scripts/install-root-helper.sh|fetch() { curl -fsSL"   # GitHub release downloads
    "scripts/k2-open"
    "crates/k2-daemon/src/update_routes.rs"                 # the update swap script
    "crates/k2-core/src/agent_hooks/install.rs"             # notify.sh
)

lint_named() { # lint_named <root> <entry>...: one line per problem; exit 1 when any
    python3 - "$@" <<'PYEOF'
import os, re, sys

root = sys.argv[1]
problems = []
CURL = re.compile(r"(^|[\s;|&(`$\"])curl\s+-")
for entry in sys.argv[2:]:
    rel, *outside = entry.split("|")
    path = os.path.join(root, rel)
    if not os.path.isfile(path):
        problems.append("%s: missing (update NAMED in the lint)" % rel)
        continue
    curls = 0
    for i, line in enumerate(open(path, encoding="utf-8", errors="replace").read().split("\n")):
        s = line.strip()
        if s.startswith("#") or s.startswith("//") or not CURL.search(line):
            continue
        curls += 1
        if any(m in line for m in outside):
            continue
        if "--noproxy '*'" not in line:
            problems.append("%s:%d: curl without --noproxy '*': %s" % (rel, i + 1, s[:100]))
    if curls == 0:
        problems.append("%s: no curl command line found (update NAMED in the lint)" % rel)
for p in problems:
    print(p)
sys.exit(1 if problems else 0)
PYEOF
}

# The Rust side: every HTTP client built in src-tauri/src or crates/*/src
# (reqwest Client::builder / ClientBuilder::new / Client::new /
# reqwest::get, ureq, hyper clients) must call .no_proxy() in the same
# statement, or in the `if ...proxy_rule::direct(url) { ... .no_proxy() }`
# right after it (k2_client::proxy_rule: the one place the rule lives),
# unless its file is listed here with how many such clients it
# builds and what they fetch. A new client, or one more in a listed file,
# fails until it skips proxies or is reviewed into this list; a listed file
# with fewer fails too, so the list stays exact. HTTP written by hand over a
# TcpStream or UnixStream (the app's /events socket, the Rust CLI's daemon
# calls) reads no proxy setting, so only the token rule below applies to it.
# Test code (a *_tests.rs or tests.rs file, a tests/ dir, a #[cfg(test)]
# module) is not read.
#
# Also on the Rust side: a string literal with ?token= or &token= in shipped
# code fails unless its file is listed in RUST_TOKEN_OK with a count.
RUST_OUTSIDE=(
    # Outside hosts: they keep the person's proxy settings.
    "crates/k2-core/src/app_templates/store.rs|1|an App template's own HTTP client, for the App's code"
    "crates/k2-core/src/compute/advertise.rs|1|the compute control plane"
    "crates/k2-core/src/compute/git.rs|1|the GitHub API"
    "crates/k2-core/src/product_feedback/wire.rs|1|the product feedback service"
    "crates/k2-core/src/push/mod.rs|5|push providers (ntfy and others)"
    "crates/k2-core/src/sim/android/sdk.rs|1|the Android SDK repository"
    "crates/k2-core/src/tunnel/cert_broker.rs|1|the K2 Connect control plane"
    "crates/k2-core/src/tunnel/cp_proxy.rs|1|the K2 Connect control plane"
    "crates/k2-core/src/tunnel/lease.rs|1|the K2 Connect control plane"
    "crates/k2-core/src/tunnel/relay_list.rs|1|the K2 Connect control plane"
    "crates/k2-core/src/tunnel/subdomains.rs|1|the K2 Connect control plane"
    "crates/k2-core/src/tunnel/unpair.rs|1|the K2 Connect control plane"
    "crates/k2-daemon/src/app_pairing/links.rs|1|an App host's app-link files"
    "crates/k2-daemon/src/app_push/sender.rs|1|phone push services"
    "crates/k2-daemon/src/apps/engine_install.rs|1|the App engine release download"
    "crates/k2-daemon/src/apps/engine_stage.rs|1|the App engine release download"
    "crates/k2-daemon/src/apps/podman_install.rs|1|the podman release download"
    "crates/k2-daemon/src/apps/relay_claims.rs|1|the K2 Connect control plane"
    "crates/k2-daemon/src/claude_auth_host.rs|1|the Claude sign-in token endpoint"
    "crates/k2-daemon/src/compute_admin.rs|1|the k2-node release download"
    "crates/k2-daemon/src/compute_local.rs|1|the k2-node release download"
    "crates/k2-daemon/src/dns/proxy.rs|1|the DNS provider API"
    "crates/k2-daemon/src/llm_accounts_runtime.rs|1|the Claude sign-in refresh grant"
    "crates/k2-daemon/src/mail/calendar_google.rs|1|the Google Calendar API"
    "crates/k2-daemon/src/mail/doctor.rs|2|the mail host's public address, as the internet sees it"
    "crates/k2-daemon/src/mail/graph.rs|1|the Microsoft Graph API"
    "crates/k2-daemon/src/mail/preflight.rs|1|public address lookups"
    "crates/k2-daemon/src/mail/sysops.rs|1|mail server release downloads"
    "crates/k2-daemon/src/root_helper/heal.rs|1|the root helper release download"
    "crates/k2-daemon/src/sew/fetch.rs|1|the voice server release download"
    "crates/k2-daemon/src/sew/ops.rs|1|the voice control plane"
    "crates/k2-daemon/src/subscription_usage.rs|1|the Claude usage API"
    "crates/k2-daemon/src/token_usage_scan.rs|1|the model price list"
    "crates/k2-daemon/src/update_routes.rs|1|the daemon release download"
    "crates/k2-daemon/src/zen_lib_routes.rs|1|the Zen library CDN"
    "src-tauri/src/commands/linux_pkg_update.rs|1|the Linux package release download"
    "src-tauri/src/commands/updater.rs|2|the app release feed and download"
    # Another K2 server over the internet, server to server: not K2's own
    # server, so the box's proxy settings still apply.
    "crates/k2-core/src/app_updates/dl_client.rs|1|an App's update downloads from its K2 server"
    "crates/k2-daemon/src/compute_trust.rs|1|a federated K2 peer"
    "crates/k2-daemon/src/federation_address.rs|1|a federated K2 peer"
    "crates/k2-daemon/src/federation_drain.rs|1|a federated K2 peer"
    "crates/k2-daemon/src/federation_routes.rs|2|a federated K2 peer"
    "crates/k2-daemon/src/federation_vault.rs|1|a federated K2 peer"
    "src-tauri/src/native_voice/provider.rs|1|HTTPS to a remote K2 host (its other client, for loopback and http, skips proxies)"
    "crates/k2-transfer-client/src/lib.rs|1|an App's transfer door, for the App's own code"
    # Services on this box that are not the daemon.
    "crates/k2-daemon/src/apps/checks.rs|1|an App's health checks (its loopback port or its public host)"
    "crates/k2-daemon/src/mail/sieve_lockdown_live.rs|1|a mail test sink"
    "crates/k2-daemon/src/voice/bff.rs|1|the voice server's client listener"
)
RUST_TOKEN_OK=(
    "crates/k2-daemon/src/apps/grace.rs|1|a notice's text names ?token=, it builds no address"
    "crates/k2-daemon/src/v1_host_sessions.rs|1|the /v1 API hands its caller a terminal WebSocket address with a short-lived stream token"
    "crates/k2-daemon/src/v1_sandboxes.rs|2|the /v1 API hands its caller a terminal WebSocket address with a short-lived stream token"
)

lint_rust() { # lint_rust <root>: one line per problem; exit 1 when any
    python3 - "$1" ${RUST_OUTSIDE[@]+"${RUST_OUTSIDE[@]}"} "--" ${RUST_TOKEN_OK[@]+"${RUST_TOKEN_OK[@]}"} <<'PYEOF'
import glob, os, re, sys

root = sys.argv[1]
rest = sys.argv[2:]
cut = rest.index("--")
def table(rows):
    t = {}
    for r in rows:
        path, n, _why = r.split("|", 2)
        t[path] = int(n)
    return t
outside, token_ok = table(rest[:cut]), table(rest[cut + 1:])
CLIENT = re.compile(r"\b(?:Client::builder\(\)|ClientBuilder::new\(\)|Client::new\(\))|\breqwest::(?:blocking::)?get\(|\bureq::(?:get|post|put|delete|request|agent|AgentBuilder)\b|\bhyper_util::client::legacy::Client\b|\bhyper::client::")
REQWEST_HINT = re.compile(r"\breqwest\b|\bureq\b|\bhyper(_util)?\b")
TOKQ = re.compile(r'"[^"\n]*[?&]token=')
TEST_FILE = re.compile(r"(^|/)(tests?|[^/]*_tests?)\.rs$|(^|/)tests?/")

def shipped_lines(path):
    lines = open(path, encoding="utf-8", errors="replace").read().split("\n")
    out = []
    for i, l in enumerate(lines):
        if l.strip().startswith("#[cfg(test)]"):
            j = i + 1
            while j < len(lines) and lines[j].strip().startswith("#["):
                j += 1
            if j < len(lines) and re.match(r"\s*(pub(\([a-z]+\))?\s+)?mod\s+\w+\s*\{", lines[j]):
                break  # the test module runs to the end of the file
        out.append((i + 1, l))
    return out

problems = []
seen_client, seen_token = {}, {}
files = sorted(set(glob.glob(os.path.join(root, "crates/*/src/**/*.rs"), recursive=True)
                   + glob.glob(os.path.join(root, "src-tauri/src/**/*.rs"), recursive=True)))
if not files:
    print("no Rust sources found under %s" % root)
    sys.exit(2)
for f in files:
    rel = os.path.relpath(f, root)
    if TEST_FILE.search(rel):
        continue
    text = open(f, encoding="utf-8", errors="replace").read()
    uses_http = bool(REQWEST_HINT.search(text))
    lines = shipped_lines(f)
    for k, (n, l) in enumerate(lines):
        s = l.strip()
        if s.startswith("//"):
            continue
        if uses_http and CLIENT.search(l):
            stmt = []
            for _, w in lines[k:k + 30]:
                stmt.append(w)
                if w.rstrip().endswith(";"):
                    break
            after = []
            for _, w in lines[k + len(stmt):k + len(stmt) + 5]:
                after.append(w)
                if w.strip() == "}":
                    break
            ruled = "proxy_rule::direct(" in "\n".join(after) and ".no_proxy()" in "\n".join(after)
            if ".no_proxy()" not in "\n".join(stmt) and not ruled:
                seen_client.setdefault(rel, []).append(n)
        if TOKQ.search(l):
            seen_token.setdefault(rel, []).append(n)

for rel, ns in sorted(seen_client.items()):
    want = outside.get(rel)
    if want is None:
        for n in ns:
            problems.append("%s:%d: an HTTP client without .no_proxy() (skip proxies, or list the file with what it fetches)" % (rel, n))
    elif len(ns) != want:
        problems.append("%s: %d HTTP clients without .no_proxy(), the list says %d (lines %s)" % (rel, len(ns), want, ",".join(map(str, ns))))
for rel, want in sorted(outside.items()):
    if rel not in seen_client:
        problems.append("%s: listed with %d HTTP clients without .no_proxy(), none found (update the list)" % (rel, want))
for rel, ns in sorted(seen_token.items()):
    want = token_ok.get(rel)
    if want is None:
        for n in ns:
            problems.append("%s:%d: a token query parameter (the credential goes in a header)" % (rel, n))
    elif len(ns) != want:
        problems.append("%s: %d token query parameters, the list says %d (lines %s)" % (rel, len(ns), want, ",".join(map(str, ns))))
for rel, want in sorted(token_ok.items()):
    if rel not in seen_token:
        problems.append("%s: listed with %d token query parameters, none found (update the list)" % (rel, want))
for p in problems:
    print(p)
sys.exit(1 if problems else 0)
PYEOF
}

# ── self test on broken and good parts ───────────────────────────────
FIX="$(mktemp -d -t k2-noproxy-lint-XXXXXX)"
trap 'rm -rf "$FIX"' EXIT
mk() { mkdir -p "$FIX/$1"; cat >"$FIX/$1/$2"; }
mk good policy.sh <<'EOF'
_k2_curl() {
    set -- --noproxy '*' "$@"
}
f() { _k2_cfg_add url "${BASE_URL}/cli/x"; _k2_cfg_auth "$TOKEN"; }
EOF
mk good py.sh <<'EOF'
g() {
    python3 - <<'PY'
import urllib.request
urllib.request.install_opener(urllib.request.build_opener(urllib.request.ProxyHandler({})))
urllib.request.urlopen(req)
PY
}
EOF
mk good help.sh <<'EOF'
help() {
    cat <<'TXT'
  curl -sS "$K2_GATEWAY_URL/cli/x?token=$TOKEN"
TXT
}
EOF
mk good update.sh <<'EOF'
u() { curl -sH "Accept: x" https://api.example.test/; }
EOF
mk no_noproxy policy.sh <<'EOF'
_k2_curl() {
    curl "$@"
}
EOF
mk bare_curl policy.sh <<'EOF'
_k2_curl() {
    set -- --noproxy '*' "$@"
}
h() { curl -s "${BASE_URL}/cli/x"; }
EOF
mk py_proxy policy.sh <<'EOF'
_k2_curl() {
    set -- --noproxy '*' "$@"
}
g() {
    python3 - <<'PY'
import urllib.request
urllib.request.urlopen(req)
PY
}
EOF
mk query_token policy.sh <<'EOF'
_k2_curl() {
    set -- --noproxy '*' "$@"
}
f() { _k2_cfg_add url "${BASE_URL}/cli/x?token=${TOKEN}"; }
EOF
mk py_query_token policy.sh <<'EOF'
_k2_curl() {
    set -- --noproxy '*' "$@"
}
g() {
    python3 - <<'PY'
import urllib.request
urllib.request.install_opener(urllib.request.build_opener(urllib.request.ProxyHandler({})))
qs = urllib.parse.urlencode(dict(params, token=token))
PY
}
EOF
self_ok=1
if ! out="$(lint "$FIX/good")"; then echo "SELF-TEST FAIL: good parts flagged: $out" >&2; self_ok=0; fi
for b in no_noproxy bare_curl py_proxy query_token py_query_token; do
    if out="$(lint "$FIX/$b")"; then echo "SELF-TEST FAIL: $b not caught" >&2; self_ok=0; fi
done
mkdir -p "$FIX/named/s"
cat >"$FIX/named/s/good.sh" <<'EOF'
# curl -s http://127.0.0.1/ in a comment is not a call
fetch() { curl -fsSL -o "$2" "$1"; }
body=$(curl -fsS --noproxy '*' "http://127.0.0.1:$PORT/boot-status")
EOF
cat >"$FIX/named/s/bad.sh" <<'EOF'
body=$(curl -fsS "http://127.0.0.1:$PORT/boot-status")
EOF
cat >"$FIX/named/s/none.sh" <<'EOF'
echo no request here
EOF
if ! out="$(lint_named "$FIX/named" "s/good.sh|fetch() { curl -fsSL")"; then echo "SELF-TEST FAIL: good named script flagged: $out" >&2; self_ok=0; fi
# good.sh without its outside-host marker: the fetch line must be caught.
for b in "s/bad.sh" "s/none.sh" "s/gone.sh" "s/good.sh"; do
    if out="$(lint_named "$FIX/named" "$b")"; then echo "SELF-TEST FAIL: named $b not caught" >&2; self_ok=0; fi
done
mkr() { mkdir -p "$(dirname "$FIX/$1/$2")"; cat >"$FIX/$1/$2"; }
mkr rust_good src-tauri/src/a.rs <<'EOF'
fn http() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(T)
        .no_proxy()
        .build()
        .expect("client");
}
EOF
mkr rust_good crates/x/src/b.rs <<'EOF'
// fetches an outside host
fn fetch() { let c = reqwest::blocking::Client::builder().build(); }
EOF
mkr rust_good crates/x/src/c.rs <<'EOF'
fn shipped() {}
#[cfg(test)]
mod tests {
    fn t() { let u = format!("http://127.0.0.1:1/cli/x?token={}", 1); let c = reqwest::Client::new(); }
}
EOF
mkr rust_good crates/x/src/d_tests.rs <<'EOF'
fn t() { let u = "/cli/x?token=abc"; let c = reqwest::Client::new(); }
EOF
mkr rust_good crates/x/src/f.rs <<'EOF'
fn probe(url: &str) -> reqwest::blocking::Client {
    let mut b = reqwest::blocking::Client::builder().timeout(T);
    if k2_client::proxy_rule::direct(url) {
        b = b.no_proxy();
    }
    b.build().expect("client")
}
EOF
mkr rust_good crates/x/src/e.rs <<'EOF'
// no HTTP crate here: this Client::new() is someone else's type
fn f() { let c = Client::new(); }
EOF
mkr rust_bad_client src-tauri/src/a.rs <<'EOF'
fn http() { let c = reqwest::blocking::Client::builder().timeout(T).build(); }
EOF
mkr rust_bad_cond src-tauri/src/a.rs <<'EOF'
fn probe(url: &str) -> reqwest::blocking::Client {
    let mut b = reqwest::blocking::Client::builder().timeout(T);
    if url.starts_with("http://127.") {
        b = b.no_proxy();
    }
    b.build().expect("client")
}
EOF
mkr rust_bad_token src-tauri/src/a.rs <<'EOF'
fn url(p: u16, t: &str) -> String { format!("http://127.0.0.1:{p}/cli/x?token={t}") }
EOF
mkr rust_bad_count crates/x/src/b.rs <<'EOF'
fn fetch() { let c = reqwest::blocking::Client::builder().build(); }
fn more() { let c = reqwest::blocking::Client::new(); }
EOF
mkr rust_stale crates/x/src/z.rs <<'EOF'
fn nothing() {}
EOF
if ! out="$(RUST_OUTSIDE=("crates/x/src/b.rs|1|fixture"); RUST_TOKEN_OK=(); lint_rust "$FIX/rust_good")"; then echo "SELF-TEST FAIL: good Rust flagged: $out" >&2; self_ok=0; fi
rust_caught() { # rust_caught <fixture> <text the problem must name> <outside row or "">
    local out
    if out="$(if [ -n "$3" ]; then RUST_OUTSIDE=("$3"); else RUST_OUTSIDE=(); fi; RUST_TOKEN_OK=(); lint_rust "$FIX/$1")"; then
        echo "SELF-TEST FAIL: $1 not caught" >&2; self_ok=0
    elif ! grep -q -- "$2" <<<"$out"; then
        echo "SELF-TEST FAIL: $1 caught for another reason: $out" >&2; self_ok=0
    fi
}
rust_caught rust_bad_client "src-tauri/src/a.rs:1: an HTTP client without .no_proxy()" ""
rust_caught rust_bad_cond "src-tauri/src/a.rs:2: an HTTP client without .no_proxy()" ""
rust_caught rust_bad_token "src-tauri/src/a.rs:1: a token query parameter" ""
rust_caught rust_bad_count "2 HTTP clients without .no_proxy(), the list says 1" "crates/x/src/b.rs|1|fixture"
rust_caught rust_stale "none found (update the list)" "crates/x/src/z.rs|1|fixture"
[ "$self_ok" = 1 ] || exit 1
echo "self test: every broken fixture caught, the good one passes"

# ── the real scan ─────────────────────────────────────────────────────
DIR="${1:-$PROJECT_ROOT/cli/src}"
if out="$(lint "$DIR")"; then
    echo "no_proxy_lint: cli/src sends no daemon request through a proxy and no credential in a query"
else
    echo "$out" >&2
    echo "no_proxy_lint: FAILED" >&2
    exit 1
fi
if out="$(lint_named "$PROJECT_ROOT" "${NAMED[@]}")"; then
    echo "no_proxy_lint: ${#NAMED[@]} scripts outside cli/src send every daemon curl with --noproxy '*'"
else
    echo "$out" >&2
    echo "no_proxy_lint: FAILED" >&2
    exit 1
fi
if out="$(lint_rust "$PROJECT_ROOT")"; then
    echo "no_proxy_lint: Rust: every HTTP client skips proxies or is on the reviewed list (${#RUST_OUTSIDE[@]} files), no token query parameter outside it (${#RUST_TOKEN_OK[@]} files)"
else
    echo "$out" >&2
    echo "no_proxy_lint: FAILED" >&2
    exit 1
fi
