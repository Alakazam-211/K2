# ── Headless Connect users (local daemon accounts) ────────────────────
# PRD: prd-headless-connect-onboarding-v1.md R2.
# These manage /cli/users/* on the LOCAL daemon (owner-token root of
# trust for the first owner; afterwards Owner/Admin session as gated
# server-side). Never put a password on argv — interactive getpass or
# --password-stdin only.

# ── k2 clone (prd-clone-carries-threads-and-vault-v1 §4.3, §4.4, CL20) ──
#
# People clone; agents propose. `to` / `from` are the same two-leg
# sequences the desktop dialog runs (clone-to.ts / clone-pull.ts): each leg
# is one daemon route, the remote leg uses --host + connect-tokens.json /
# $K2_REMOTE_TOKEN like `k2 talk`. Works headless on Linux. Output never
# prints a secret value: --json prints names, choices and counts.
#
# Exit codes: 0 ok · 2 bad usage / bad request · 3 refused or unreachable
# (clone_not_allowed, clone_personal_computers_off, confirm_required, …).

_clone_usage() {
    cat <<'EOF'
k2 clone — copy an agent (workspace + chat history + Thread) to another server

People:
  k2 clone preview <ws> [--host H] [--threads-since N] [--json]
                                   what would travel (names only)
  k2 clone to <host> <ws> [--dest DIR] [flags]      push a workspace from this server
  k2 clone from <host>:<ws> [--dest DIR] [flags]    pull one to this computer
  k2 clone here <ws> [--dest DIR] [flags]           a second copy on this server
  k2 clone <ws> --dest DIR [flags]                  same as `here`
  k2 clone proposals [--all] [--json]               agent proposals waiting for a person
  k2 clone approve <id> [--yes]                     approve (asks you to confirm) and run it
  k2 clone deny <id> [--reason TEXT]

Agents (a person approves):
  k2 clone propose --to <host|here> [flags] --why "…"
  k2 clone proposals --mine        k2 clone withdraw <id>

Flags:
  --no-threads                 leave the Thread behind (default: bring it)
  --threads-since N            only the last N days of Thread
  --copy A,B  --blank C  --skip D
                               per-secret choices (names only). Without them a
                               terminal asks per secret; otherwise defaults:
                               API keys copy, sign-ins skip, server/box keys blank.
                               Copying needs an Owner/Admin confirm.
  --own                        this is my own server (keeps "given by a person")
  --live-only                  only the newest chat transcript (default: all)
  --env-files                  also send .env files as files (asks you to confirm;
                               better: k2 vault import .env first)
  --token T                    remote token (else connect-tokens.json / $K2_REMOTE_TOKEN)
  --yes                        don't ask; take defaults and flags as given
  --json                       names, choices and counts as JSON

Agents can't clone: `k2 clone propose` holds the plan for a person, who runs
`k2 clone approve <id>` from a terminal outside K2 (or approves in K2).
Exit: 0 ok · 2 bad usage · 3 refused or unreachable.
EOF
}

_clone_die() {
    local code="$1" hint="$2" rc="${3:-3}"
    python3 -c 'import json,sys; print(json.dumps({"error": sys.argv[1], "hint": sys.argv[2]}))' "$code" "$hint" >&2
    exit "$rc"
}

# People-only verbs refuse a passport (an agent, or a K2 terminal).
_clone_person_guard() {
    if _compute_in_session || [ -n "${K2_HOOK_TOKEN:-}${K2SO_HOOK_TOKEN:-}" ]; then
        _clone_die clone_agents_propose "agents can't clone; propose it and a person approves: k2 clone propose --to <host|here> --why \"…\""
    fi
}

# The engine: preview, push, pull, here, show, approve. Python so the
# chunked transfer and the JSON stay in one place. Tokens and passwords ride
# the environment and /dev/tty, never argv.
_clone_py() {
    K2C_LOCAL_URL="$BASE_URL" K2C_LOCAL_TOKEN="$TOKEN" K2C_K2_HOME="$K2_HOME" \
    K2C_VAULT_SOCK="$(_vault_sock)" K2C_TTY="$( { [ -t 0 ] || [ -t 2 ]; } && [ -r /dev/tty ] && echo 1 || echo 0)" \
    python3 - "$@" <<'PY'
import base64, getpass, http.client, json, os, socket, sys, time, uuid
import urllib.error, urllib.parse, urllib.request

LOCAL = ("local", os.environ["K2C_LOCAL_URL"], os.environ["K2C_LOCAL_TOKEN"])
REMOTE = ("remote", os.environ.get("K2C_REMOTE_URL", ""), os.environ.get("K2C_REMOTE_TOKEN", ""))
OPTS = json.loads(os.environ.get("K2C_OPTS") or "{}")
K2_HOME = os.environ["K2C_K2_HOME"]
VSOCK = os.environ.get("K2C_VAULT_SOCK", "")
TTY = os.environ.get("K2C_TTY") == "1" and not OPTS.get("yes")
JSON_OUT = bool(OPTS.get("json"))
CHUNK = 8 * 1024 * 1024


class Fail(Exception):
    def __init__(self, code, hint, rc=3):
        super().__init__(hint)
        self.code, self.hint, self.rc = code, hint, rc


def say(msg):
    if not JSON_OUT:
        print(msg, file=sys.stderr)


def refusal(status, v):
    e = v.get("error") if isinstance(v, dict) else None
    if isinstance(e, dict):
        code, hint = e.get("code", "error"), e.get("hint", "")
    else:
        code = (v.get("code") if isinstance(v, dict) else None) or "error"
        hint = (v.get("hint") if isinstance(v, dict) else None) or (e if isinstance(e, str) else "") or "refused (%s)" % status
    return Fail(code, hint, 2 if status in (400, 413) else 3)


def call(side, method, route, body=None, params=None, timeout=120):
    name, base, tok = side
    if not base:
        raise Fail("usage", "no %s server" % name, 2)
    q = {"token": tok}
    q.update({k: v for k, v in (params or {}).items() if v is not None})
    url = base + route + "?" + urllib.parse.urlencode(q)
    data = None
    if method == "POST":
        data = json.dumps(body if body is not None else {}).encode()
    req = urllib.request.Request(url, data=data, method=method, headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            status, raw = r.status, r.read()
    except urllib.error.HTTPError as e:
        status, raw = e.code, e.read()
    except (urllib.error.URLError, OSError) as e:
        raise Fail("daemon_unreachable", "%s server %s: %s" % (name, base, getattr(e, "reason", e)), 3)
    try:
        v = json.loads(raw or b"{}")
    except ValueError:
        v = {"error": raw[:200].decode(errors="replace")}
    if status != 200:
        raise refusal(status, v)
    return v


class _UConn(http.client.HTTPConnection):
    def __init__(self, path, timeout):
        super().__init__("localhost", timeout=timeout)
        self._p = path

    def connect(self):
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.settimeout(self.timeout)
        s.connect(self._p)
        self.sock = s


def vault_sock(route, body):
    c = _UConn(VSOCK, 180)
    try:
        c.request("POST", "/cli/vault/" + route, body=json.dumps(body),
                  headers={"Content-Type": "application/json", "X-K2-Tty": "1" if TTY else "0"})
        r = c.getresponse()
        status, raw = r.status, r.read()
    except OSError as e:
        raise Fail("vault_unreachable", "vault socket %s: %s" % (VSOCK, e), 3)
    v = json.loads(raw or b"{}")
    if status != 200:
        raise refusal(status, v)
    return v


def password(prompt):
    if not TTY:
        raise Fail("confirm_needs_a_person", "this needs your confirm: run it from a terminal (not --yes, not a pipe)", 3)
    return getpass.getpass(prompt)


def confirm(side, action, secret_id, label):
    """A fresh vault confirm token on `side`'s daemon (Touch ID or a password)."""
    local = side[0] == "local" and VSOCK and os.path.exists(VSOCK)
    start = {"action": action}
    if local:
        methods = vault_sock("confirm/start", start).get("methods") or []
    else:
        methods = call(side, "POST", "/cli/vault/confirm/start", start).get("methods") or []
    body = {"action": action, "secretId": secret_id}
    if local and "local-auth" in methods:
        say("Approve on this Mac (Touch ID or your login password): %s" % label)
        body.update(method="local-auth", label=label)
    elif "connect-password" in methods:
        body.update(method="connect-password", password=password("Your password on %s (%s): " % (side[1], label)))
    elif "vault-password" in methods:
        body.update(method="vault-password", password=password("Vault password (%s): " % label))
    else:
        raise Fail("no_confirm_method", "set a vault password first: k2 vault confirm-password set (from a terminal outside K2)", 3)
    if local:
        tok = vault_sock("confirm/finish", body)["token"]
    else:
        tok = call(side, "POST", "/cli/vault/confirm/finish", body)["token"]
    body.pop("password", None)
    return tok


CLONE_ID = str(uuid.uuid4())


def vault_target(side, label):
    """The destination vault for a carry (`GET /cli/vault/recipient`), or
    None (an older K2, or its vault is closed): then no secret travels."""
    try:
        r = call(side, "GET", "/cli/vault/recipient")
    except Fail as e:
        say("(secrets stay behind: %s's vault answered %s)" % (label, e.code))
        return None
    if not r.get("recipient") or not r.get("vaultId"):
        return None
    return {"recipient": r["recipient"], "vault_id": r["vaultId"], "label": label}


def carry_ask(path, target, choices, env_files):
    """Slice A's CarryAsk: the plan the clone-carry confirm is bound to."""
    a = {"project_path": path, "clone_id": CLONE_ID, "secrets": choices,
         "own_box_move": bool(OPTS.get("own")), "send_env_files": bool(env_files)}
    if target:
        a["vault_target"] = target
    return a


def carry_confirm_call(side, ask, pv, proposal=None):
    """One person confirm for exactly this plan (copies and/or .env files):
    vault.sock on this computer, else the source's /cli/clone/vault/confirm
    with the person's password. With `proposal` (0.46.1) the same single
    confirm also approves that clone proposal: the answer carries
    `approveToken` next to `token`."""
    local = side[0] == "local" and VSOCK and os.path.exists(VSOCK)
    reason = ((pv.get("secrets") or {}).get("confirm") or {}).get("reason") or "copy secrets in a clone"
    if proposal:
        reason = "approve the clone proposal and " + reason
    if local:
        methods = vault_sock("confirm/start", {"action": "clone-carry"}).get("methods") or []
        body = {"action": "clone-carry", "clonePlan": ask}
        if proposal:
            body["proposal"] = proposal
        if "local-auth" in methods:
            say("Approve on this Mac (Touch ID or your login password): %s" % reason)
            body["method"] = "local-auth"
        elif "vault-password" in methods:
            body.update(method="vault-password", password=password("Vault password (%s): " % reason))
        else:
            raise Fail("no_confirm_method", "set a vault password first: k2 vault confirm-password set (from a terminal outside K2)", 3)
        r = vault_sock("confirm/finish", body)
        body.pop("password", None)
        return r
    methods = ((pv.get("secrets") or {}).get("confirm") or {}).get("methods") or []
    body = dict(ask)
    if proposal:
        body["proposal"] = proposal
    if "connect-password" in methods:
        body.update(method="connect-password", password=password("Your password on %s (%s): " % (side[1], reason)))
    elif "vault-password" in methods:
        body.update(method="vault-password", password=password("Vault password on %s (%s): " % (side[1], reason)))
    else:
        raise Fail("no_confirm_method", "%s offers no password confirm for you (an Owner or Admin login, or a vault password, is needed)" % side[1], 3)
    r = call(side, "POST", "/cli/clone/vault/confirm", body)
    body.pop("password", None)
    return r


def carry_confirm(side, ask, pv):
    """A fresh clone-carry confirm for exactly this plan."""
    return carry_confirm_call(side, ask, pv)["token"]


APPROVED = {}


def approve_proposal(approve_token):
    """Approve OPTS["proposal"] with a clone-approve token (once)."""
    pid = OPTS["proposal"]
    call(LOCAL, "POST", "/cli/clone-proposal/approve", {"proposal": pid, "confirmToken": approve_token})
    APPROVED[pid] = True


def threads_value():
    if OPTS.get("noThreads"):
        return "none"
    if OPTS.get("threadsSince"):
        return {"sinceDays": int(OPTS["threadsSince"])}
    return "all"


def ask_choice(row, default):
    allowed = ["copy", "blank", "skip"] if row.get("copyable", True) else ["blank", "skip"]
    note = (" — " + row["note"]) if row.get("note") else ""
    with open("/dev/tty", "r+") as t:
        while True:
            t.write("  %-24s %-10s %s [%s]%s: " % (row["name"], row.get("kind", ""), "/".join(allowed), default, note))
            t.flush()
            a = t.readline().strip().lower()
            if not a:
                return default
            for c in allowed:
                if c.startswith(a):
                    return c
            t.write("    choose %s\n" % ", ".join(allowed))


def choose_secrets(preview):
    sec = preview.get("secrets") or {}
    rows = (sec.get("rows") or []) if sec.get("available") else []
    can_copy = bool(preview.get("callerCanCopySecrets"))
    flags = {}
    for key in ("copy", "blank", "skip"):
        for n in OPTS.get(key) or []:
            if n in flags and flags[n] != key:
                raise Fail("usage", "'%s' is named twice with different choices" % n, 2)
            flags[n] = key
    if flags and not rows:
        raise Fail("clone_secrets_unavailable", "no secrets to choose here (none, or the destination's vault can't take a carry): drop --copy/--blank/--skip", 2)
    names = {r["name"] for r in rows}
    for n in flags:
        if n not in names:
            raise Fail("usage", "no secret '%s' in this agent's plan (k2 clone preview)" % n, 2)
    choices = {}
    for r in rows:
        d = r.get("choice") or r.get("default") or "skip"
        if d == "copy" and not (can_copy and r.get("copyable", False)):
            d = "blank"
        choices[r["name"]] = flags.get(r["name"], d)
    if rows and not flags and TTY:
        say("Secrets (names only). Enter keeps the default.")
        for r in rows:
            choices[r["name"]] = ask_choice(dict(r, copyable=r.get("copyable", False) and can_copy), choices[r["name"]])
    for r in rows:
        if choices[r["name"]] == "copy" and not r.get("copyable", False):
            raise Fail("clone_secrets_owner_or_admin" if not can_copy else "usage",
                       "'%s' can't be copied by you (%s)" % (r["name"], r.get("note") or "copying secrets needs an Owner or Admin"),
                       3 if not can_copy else 2)
    return choices


def preview(side, ws, target=None):
    body = {"workspace": ws} if not ws.startswith("/") else {"project_path": ws}
    if OPTS.get("threadsSince"):
        body["sinceDays"] = int(OPTS["threadsSince"])
    body["clone_id"] = CLONE_ID
    body["own_box_move"] = bool(OPTS.get("own"))
    if target:
        body["vault_target"] = target
    return call(side, "POST", "/cli/clone/preview", body, timeout=600)


def env_files_wanted(pv):
    if not OPTS.get("envFiles"):
        return False
    if not pv.get("envFiles"):
        say("(no .env files to send)")
        return False
    return True


def bundle_body(side, pv, choices, target, with_vault=True):
    """bundle/pack body: Threads, the clone id, the vault half and the
    clone-carry confirm when anything copies or .env files travel."""
    path = pv["workspace"]["path"]
    env = env_files_wanted(pv)
    body = {"project_path": path, "live_only": bool(OPTS.get("liveOnly")), "threads": threads_value(),
            "clone_id": CLONE_ID, "carry_secrets": env}
    if OPTS.get("proposal"):
        body["proposal_id"] = OPTS["proposal"]
    ask = carry_ask(path, target if with_vault else None, choices if with_vault else {}, env)
    if with_vault and target:
        body["vault_target"] = target
        body["secrets"] = choices
        body["own_box_move"] = bool(OPTS.get("own"))
    copies = with_vault and any(c == "copy" for c in choices.values())
    if copies and not target:
        raise Fail("clone_carry_unavailable", "the destination can't take a vault carry (older K2 or its vault is closed); choose blank or skip", 3)
    pid = OPTS.get("proposal")
    if pid and not APPROVED.get(pid):
        # k2 clone approve (0.46.1): ONE confirm approves the proposal and,
        # when this plan copies secrets or sends .env files, covers them too.
        if copies or env:
            r = carry_confirm_call(side, ask, pv, proposal=pid)
            approve_proposal(r["approveToken"])
            body["confirm_token"] = r["token"]
        else:
            approve_proposal(confirm(LOCAL, "clone-approve", pid, "approve " + (OPTS.get("proposalSummary") or pid)[:60]))
        return body
    if copies or env:
        body["confirm_token"] = carry_confirm(side, ask, pv)
    return body


def upload(side, local_path, dest_dir):
    size = os.path.getsize(local_path)
    name = os.path.basename(local_path)
    uid = "clone-cli-%s" % uuid.uuid4().hex[:12]
    final, off = None, 0
    with open(local_path, "rb") as f:
        while True:
            data = f.read(CHUNK)
            last = off + len(data) >= size
            body = {"upload_id": uid, "dir": dest_dir, "filename": name, "offset": off,
                    "base64": base64.b64encode(data).decode(), "is_last": last}
            if off == 0:
                body["total_bytes"] = size
            r = call(side, "POST", "/cli/fs/upload-chunk", body, timeout=600)
            off += len(data)
            if last:
                final = r.get("path")
                break
    if not final:
        raise Fail("upload_failed", "the upload finished without a path", 3)
    return final


def download(side, remote_path, size):
    tmp = os.path.join(K2_HOME, "clone-tmp")
    os.makedirs(tmp, exist_ok=True)
    out = os.path.join(tmp, os.path.basename(remote_path))
    part = out + ".part"
    off = 0
    with open(part, "wb") as f:
        while True:
            r = call(side, "GET", "/cli/fs/read-range", params={"path": remote_path, "offset": off, "len": CHUNK}, timeout=600)
            data = base64.b64decode(r.get("base64") or "")
            f.write(data)
            off += len(data)
            if r.get("eof") or not data:
                break
    if size and off != size:
        os.unlink(part)
        raise Fail("download_failed", "got %d of %d bytes" % (off, size), 3)
    os.chmod(part, 0o600)
    os.replace(part, out)
    return out


def remote_home(side):
    info = call(side, "GET", "/cli/fs/info")
    sep = info.get("separator") or "/"
    home = (info.get("home") or "").rstrip(sep)
    if not home:
        raise Fail("remote_home_unknown", "the remote server didn't say its home folder; pass --dest", 3)
    return home, sep


def result_json(mode, pv, choices, unpacked, extra=None):
    out = {
        "ok": True,
        "mode": mode,
        "cloneId": CLONE_ID,
        "source": pv["workspace"]["handle"],
        "destPath": unpacked.get("dest_path"),
        "project": {k: (unpacked.get("project") or {}).get(k) for k in ("id", "name", "path")},
        "threads": threads_value(),
        "secrets": choices,
    }
    for k in ("threads_imported", "threads_skipped", "vault_imported", "vault_skipped", "vault_grants",
              "vault_cards_posted", "vault_cards_deferred", "vault_cards_overflow", "vault_error", "threads_error"):
        if unpacked.get(k) not in (None, [], ""):
            out[k] = unpacked[k]
    out.update(extra or {})
    return out


def show_plan(pv, where):
    if JSON_OUT:
        return
    w = pv["workspace"]
    h = pv.get("history") or {}
    say("Clone %s (%s) %s" % (w["handle"], w["path"], where))
    say("  files: %s · chat transcripts: %s" % (h.get("files"), h.get("sessions")))
    t = pv.get("threads") or {}
    if OPTS.get("noThreads"):
        say("  Thread: left behind (--no-threads)")
    elif t.get("available"):
        say("  Thread: %s conversation(s), %s message(s)%s. Chatter never travels." % (
            t.get("conversations"), t.get("docs"),
            (" from " + ", ".join(t["participants"])) if t.get("participants") else ""))
    else:
        say("  Thread: brought along (counts need the Thread update on this server). Chatter never travels.")
    if pv.get("envFiles"):
        say("  .env files held back: %s%s" % (", ".join(pv["envFiles"]),
            "" if OPTS.get("envFiles") else " (move them into the vault: k2 vault import .env)"))


def push(ws):
    target = vault_target(REMOTE, REMOTE[1].split("://")[-1])
    pv = preview(LOCAL, ws, target)
    show_plan(pv, "to %s" % REMOTE[1])
    choices = choose_secrets(pv)
    body = bundle_body(LOCAL, pv, choices, target)
    say("Building the bundle…")
    b = call(LOCAL, "POST", "/cli/clone/bundle", body, timeout=3600)
    home, sep = remote_home(REMOTE)
    dest = OPTS.get("dest") or home
    say("Uploading %s…" % os.path.basename(b["bundle_path"]))
    remote_bundle = upload(REMOTE, b["bundle_path"], home + sep + ".k2" + sep + "clone-tmp")
    try:
        os.unlink(b["bundle_path"])
    except OSError:
        pass
    ub = {"bundle_path": remote_bundle, "dest_parent": dest}
    if b.get("vault_carry_b64"):
        ub["vault_carry_b64"] = b["vault_carry_b64"]
    say("Unpacking on %s…" % REMOTE[1])
    u = call(REMOTE, "POST", "/cli/clone/unpack", ub, timeout=3600)
    return result_json("to", pv, choices, u, {"host": REMOTE[1]})


def pull(ws):
    target = vault_target(LOCAL, "this computer")
    pv = preview(REMOTE, ws, target)
    show_plan(pv, "from %s to this computer" % REMOTE[1])
    choices = choose_secrets(pv)
    body = bundle_body(REMOTE, pv, choices, target)
    say("Packing on %s…" % REMOTE[1])
    job = call(REMOTE, "POST", "/cli/clone/pack", body)["job_id"]
    deadline = time.time() + 3600
    while True:
        st = call(REMOTE, "GET", "/cli/clone/pack-status", params={"job_id": job})
        if st.get("phase") == "done":
            break
        if st.get("phase") == "failed":
            raise Fail("pack_failed", st.get("error") or "the pack failed", 3)
        if time.time() > deadline:
            raise Fail("pack_timeout", "the pack took over an hour", 3)
        time.sleep(0.5)
    say("Downloading %s bytes…" % st.get("size_bytes"))
    try:
        local_bundle = download(REMOTE, st["bundle_path"], st.get("size_bytes") or 0)
    finally:
        try:
            call(REMOTE, "POST", "/cli/clone/pack-cleanup", {"job_id": job})
        except Fail:
            pass
    ub = {"bundle_path": local_bundle, "dest_parent": OPTS.get("dest") or os.path.expanduser("~")}
    if st.get("vault_carry_b64"):
        ub["vault_carry_b64"] = st["vault_carry_b64"]
    say("Unpacking here…")
    u = call(LOCAL, "POST", "/cli/clone/unpack", ub, timeout=3600)
    return result_json("from", pv, choices, u, {"host": REMOTE[1]})


def here(ws):
    """A second copy on this server: the bundle carries files and Threads;
    secrets are re-encrypted in place afterwards (/cli/clone/vault/copy)."""
    target = vault_target(LOCAL, "a second copy on this server")
    pv = preview(LOCAL, ws, target)
    show_plan(pv, "to a second copy on this server")
    choices = choose_secrets(pv)
    body = bundle_body(LOCAL, pv, choices, target, with_vault=False)
    b = call(LOCAL, "POST", "/cli/clone/bundle", body, timeout=3600)
    dest = OPTS.get("dest") or os.path.dirname(pv["workspace"]["path"].rstrip("/"))
    u = call(LOCAL, "POST", "/cli/clone/unpack", {"bundle_path": b["bundle_path"], "dest_parent": dest}, timeout=3600)
    extra = {}
    if target and any(c != "skip" for c in choices.values()):
        ask = carry_ask(pv["workspace"]["path"], target, choices, False)
        cbody = dict(ask, new_project_path=u.get("dest_path"))
        if any(c == "copy" for c in choices.values()):
            cbody["confirm_token"] = carry_confirm(LOCAL, ask, pv)
        extra["vault"] = call(LOCAL, "POST", "/cli/clone/vault/copy", cbody, timeout=600)
    return result_json("here", pv, choices, u, extra)


def find_proposal(token):
    rows = call(LOCAL, "GET", "/cli/clone-proposal/list", params={"status": "all"}).get("proposals") or []
    hit = [r for r in rows if r["id"] == token] or [r for r in rows if r["id"].startswith(token)]
    if len(hit) != 1:
        raise Fail("proposal_not_found", "no single clone proposal matches '%s' (k2 clone proposals)" % token, 3)
    return hit[0]


def approve(token):
    p = find_proposal(token)
    if p["status"] != "open":
        raise Fail("proposal_not_open", "%s is %s" % (p["id"], p["status"]), 3)
    say("%s: %s" % (p["id"], p["summary"]))
    say("asked by %s%s" % (p["proposedBy"], (": " + p["why"]) if p.get("why") else ""))
    # The approve happens inside the run, right before the bundle, with the
    # same single confirm that covers copying secrets (bundle_body).
    run = {"workspacePath": p.get("workspacePath"), "target": p["target"]}
    if not run["workspacePath"]:
        raise Fail("workspace_not_found", "the proposal's workspace isn't registered here any more", 3)
    o = p.get("options") or {}
    OPTS.update({
        "proposal": p["id"], "proposalSummary": p.get("summary"),
        "noThreads": not o.get("threads", True), "threadsSince": o.get("sinceDays"),
        "liveOnly": not o.get("allHistory", True), "dest": OPTS.get("dest") or o.get("dest"),
        "copy": [n for n, c in (o.get("secrets") or {}).items() if c == "copy"],
        "blank": [n for n, c in (o.get("secrets") or {}).items() if c == "blank"],
        "skip": [n for n, c in (o.get("secrets") or {}).items() if c == "skip"],
    })
    try:
        res = here(run["workspacePath"]) if run["target"] == "here" else push(run["workspacePath"])
    except Fail as e:
        if APPROVED.get(p["id"]):
            call(LOCAL, "POST", "/cli/clone-proposal/finish", {"proposal": p["id"], "ok": False, "detail": e.code})
        raise
    call(LOCAL, "POST", "/cli/clone-proposal/finish", {"proposal": p["id"], "ok": True})
    res["proposal"] = p["id"]
    return res


def main():
    verb = sys.argv[1]
    arg = sys.argv[2] if len(sys.argv) > 2 else ""
    try:
        if verb == "preview":
            out = preview(REMOTE if REMOTE[1] else LOCAL, arg)
            if JSON_OUT:
                print(json.dumps(out, indent=2))
            else:
                show_plan(out, "(preview)")
                sec = out.get("secrets") or {}
                for r in sec.get("rows") or []:
                    print("  %-24s %-10s %-9s default %s%s" % (r["name"], r.get("kind", ""), r.get("group", ""), r.get("default"),
                          (" — " + r["note"]) if r.get("note") else ""), file=sys.stderr)
                if sec.get("notCarried"):
                    print("  not carried: " + ", ".join(sec["notCarried"]), file=sys.stderr)
                if not out.get("callerCanCopySecrets"):
                    print("  copying secrets needs an Owner or Admin", file=sys.stderr)
            return 0
        if verb == "show":
            p = find_proposal(arg)
            print(json.dumps({"id": p["id"], "target": p["target"], "status": p["status"]}))
            return 0
        res = {"push": push, "pull": pull, "here": here, "approve": approve}[verb](arg)
    except Fail as e:
        print(json.dumps({"error": e.code, "hint": e.hint}), file=sys.stderr)
        return e.rc
    except KeyboardInterrupt:
        print(json.dumps({"error": "cancelled", "hint": "stopped"}), file=sys.stderr)
        return 3
    if JSON_OUT:
        print(json.dumps(res, indent=2))
    else:
        print("Cloned %s → %s%s" % (res["source"], res.get("destPath"), (" on " + res["host"]) if res.get("host") else ""))
        if res.get("secrets"):
            counts = {}
            for c in res["secrets"].values():
                counts[c] = counts.get(c, 0) + 1
            print("  secrets: " + " · ".join("%s %d" % (k, v) for k, v in sorted(counts.items())))
        for k in ("threads_imported", "vault_imported", "cards_posted"):
            if k in res:
                print("  %s: %s" % (k.replace("_", " "), res[k]))
    return 0


sys.exit(main())
PY
}

# Parse the shared flags into K2C_OPTS (JSON). Leftover positionals land in
# _CLONE_ARGS.
_clone_parse() {
    _CLONE_ARGS=()
    _CLONE_TOKEN=""
    local copy="" blank="" skip="" since="" dest="" why="" to="" ws="" reason=""
    local no_threads=0 own=0 live_only=0 env_files=0 yes=0 json=0 mine=0 all=0
    _CLONE_HOST=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --no-threads) no_threads=1; shift ;;
            --threads-since|--since-days) since="${2:-}"; shift 2 ;;
            --copy) copy="${copy}${copy:+,}${2:-}"; shift 2 ;;
            --blank) blank="${blank}${blank:+,}${2:-}"; shift 2 ;;
            --skip) skip="${skip}${skip:+,}${2:-}"; shift 2 ;;
            --own) own=1; shift ;;
            --live-only) live_only=1; shift ;;
            --env-files) env_files=1; shift ;;
            --dest) dest="${2:-}"; shift 2 ;;
            --host) _CLONE_HOST="${2:-}"; shift 2 ;;
            --token) _CLONE_TOKEN="${2:-}"; shift 2 ;;
            --to) to="${2:-}"; shift 2 ;;
            --why) why="${2:-}"; shift 2 ;;
            --reason) reason="${2:-}"; shift 2 ;;
            --workspace) ws="${2:-}"; shift 2 ;;
            --yes|-y) yes=1; shift ;;
            --json) json=1; shift ;;
            --mine) mine=1; shift ;;
            --all) all=1; shift ;;
            -h|--help) _clone_usage; exit 0 ;;
            -*) _clone_die usage "unknown flag '$1' (k2 clone --help)" 2 ;;
            *) _CLONE_ARGS+=("$1"); shift ;;
        esac
    done
    if [ -n "$since" ]; then
        case "$since" in ''|*[!0-9]*) _clone_die usage "--threads-since takes a number of days" 2 ;; esac
    fi
    K2C_OPTS="$(K2C_copy="$copy" K2C_blank="$blank" K2C_skip="$skip" K2C_since="$since" K2C_dest="$dest" \
        K2C_why="$why" K2C_to="$to" K2C_ws="$ws" K2C_reason="$reason" \
        K2C_flags="$no_threads $own $live_only $env_files $yes $json $mine $all" python3 -c '
import json, os
e = os.environ
f = [x == "1" for x in e["K2C_flags"].split()]
lst = lambda k: [n.strip() for n in e.get(k, "").split(",") if n.strip()]
o = {"noThreads": f[0], "own": f[1], "liveOnly": f[2], "envFiles": f[3], "yes": f[4], "json": f[5], "mine": f[6], "all": f[7],
     "copy": lst("K2C_copy"), "blank": lst("K2C_blank"), "skip": lst("K2C_skip")}
for k, n in (("threadsSince", "K2C_since"), ("dest", "K2C_dest"), ("why", "K2C_why"), ("to", "K2C_to"),
             ("workspace", "K2C_ws"), ("reason", "K2C_reason")):
    if e.get(n):
        o[k] = int(e[n]) if k == "threadsSince" else e[n]
print(json.dumps(o))')"
    export K2C_OPTS
}

_clone_opt() {
    printf '%s' "$K2C_OPTS" | python3 -c 'import json,sys; v=json.load(sys.stdin).get(sys.argv[1]); print("" if v in (None, False) else (json.dumps(v) if isinstance(v,(list,dict)) else v))' "$1"
}

# Point K2C_REMOTE_* at a Connect host (token from --token / env / file).
_clone_remote() {
    local host="$1" tok
    tok="$(_remote_token_for_host "$host" "$_CLONE_TOKEN")"
    [ -n "$tok" ] || _clone_die no_remote_token "no token for '$host': sign in to it in K2 with Remember on, pass --token, set \$K2_REMOTE_TOKEN, or add it to ${K2_HOME}/connect-tokens.json"
    case "$host" in
        http://*|https://*) K2C_REMOTE_URL="${host%/}" ;;
        *) K2C_REMOTE_URL="https://${host}" ;;
    esac
    K2C_REMOTE_TOKEN="$tok"
    export K2C_REMOTE_URL K2C_REMOTE_TOKEN
}

cmd_clone() {
    local sub="${1:-}"
    case "$sub" in
        ""|-h|--help|help) _clone_usage; return 0 ;;
    esac
    shift
    case "$sub" in
        preview)
            _clone_parse "$@"
            [ "${#_CLONE_ARGS[@]}" -eq 1 ] || _clone_die usage "usage: k2 clone preview <ws> [--host H]" 2
            _clone_person_guard
            [ -n "$_CLONE_HOST" ] && _clone_remote "$_CLONE_HOST"
            _clone_py preview "${_CLONE_ARGS[0]}"
            ;;
        to)
            _clone_parse "$@"
            [ "${#_CLONE_ARGS[@]}" -eq 2 ] || _clone_die usage "usage: k2 clone to <host> <ws> [--dest DIR]" 2
            _clone_person_guard
            _clone_remote "${_CLONE_ARGS[0]}"
            _clone_py push "${_CLONE_ARGS[1]}"
            ;;
        from)
            _clone_parse "$@"
            [ "${#_CLONE_ARGS[@]}" -eq 1 ] || _clone_die usage "usage: k2 clone from <host>:<ws> [--dest DIR]" 2
            local spec="${_CLONE_ARGS[0]}"
            case "$spec" in *:*) ;; *) _clone_die usage "usage: k2 clone from <host>:<ws>" 2 ;; esac
            _clone_person_guard
            # The workspace is after the LAST colon (a URL host has its own).
            _clone_remote "${spec%:*}"
            _clone_py pull "${spec##*:}"
            ;;
        here)
            _clone_parse "$@"
            [ "${#_CLONE_ARGS[@]}" -eq 1 ] || _clone_die usage "usage: k2 clone here <ws> [--dest DIR]" 2
            _clone_person_guard
            _clone_py here "${_CLONE_ARGS[0]}"
            ;;
        proposals|proposal)
            _clone_parse "$@"
            local q="status=open"
            [ "$(_clone_opt all)" = "True" ] && q="status=all"
            _compute_call GET /cli/clone-proposal/list "$q"
            if [ "$(_clone_opt json)" = "True" ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
rs = json.load(sys.stdin).get("proposals") or []
if not rs:
    print("No clone proposals."); sys.exit(0)
for r in rs:
    print("%s  %-9s %s" % (r["id"], r["status"], r["summary"]))
    print("    by %s%s" % (r["proposedBy"], (": " + r["why"]) if r.get("why") else ""))
    if r.get("decidedBy"):
        print("    %s by %s" % (r["status"], r["decidedBy"]))'
            ;;
        propose)
            _clone_parse "$@"
            [ "${#_CLONE_ARGS[@]}" -eq 0 ] || _clone_die usage "usage: k2 clone propose --to <host|here> --why \"…\"" 2
            local body
            body="$(printf '%s' "$K2C_OPTS" | python3 -c '
import json, sys
o = json.load(sys.stdin)
if not o.get("to"):
    print("usage: --to <host|here>", file=sys.stderr); sys.exit(2)
b = {"to": o["to"], "threads": not o.get("noThreads"), "allHistory": not o.get("liveOnly"),
     "copy": o.get("copy") or [], "blank": o.get("blank") or [], "skip": o.get("skip") or []}
for k in ("why", "dest", "workspace"):
    if o.get(k): b[k] = o[k]
if o.get("threadsSince"): b["sinceDays"] = o["threadsSince"]
print(json.dumps(b))')" || exit 2
            _compute_call POST /cli/clone-proposal/new "$body"
            if [ "$(_clone_opt json)" = "True" ]; then printf '%s\n' "$_CP_BODY"; return 0; fi
            printf '%s' "$_CP_BODY" | python3 -c '
import json, sys
v = json.load(sys.stdin)
p = v["proposal"]
print("%s held: %s" % (p["id"], p["summary"]))
print("  " + v.get("hint", ""))'
            ;;
        withdraw)
            _clone_parse "$@"
            [ "${#_CLONE_ARGS[@]}" -eq 1 ] || _clone_die usage "usage: k2 clone withdraw <id>" 2
            _compute_call POST /cli/clone-proposal/withdraw "$(_compute_json K2_CJ_proposal="${_CLONE_ARGS[0]}")"
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; p=json.load(sys.stdin)["proposal"]; print("%s %s" % (p["id"], p["status"]))'
            ;;
        deny)
            _clone_parse "$@"
            [ "${#_CLONE_ARGS[@]}" -eq 1 ] || _clone_die usage "usage: k2 clone deny <id> [--reason TEXT]" 2
            _clone_person_guard
            _compute_call POST /cli/clone-proposal/deny "$(_compute_json K2_CJ_proposal="${_CLONE_ARGS[0]}" K2_CJ_reason="$(_clone_opt reason)")" tcp
            printf '%s' "$_CP_BODY" | python3 -c 'import json,sys; p=json.load(sys.stdin)["proposal"]; print("%s %s" % (p["id"], p["status"]))'
            ;;
        approve)
            _clone_parse "$@"
            [ "${#_CLONE_ARGS[@]}" -eq 1 ] || _clone_die usage "usage: k2 clone approve <id>" 2
            _clone_person_guard
            local shown target
            shown="$(_clone_py show "${_CLONE_ARGS[0]}")" || exit $?
            target="$(printf '%s' "$shown" | python3 -c 'import json,sys; print(json.load(sys.stdin)["target"])')"
            [ "$target" = "here" ] || _clone_remote "$target"
            _clone_py approve "${_CLONE_ARGS[0]}"
            ;;
        *)
            # Bare `k2 clone <ws> --dest DIR` = `here` (dup PRD A4 alias).
            _clone_parse "$sub" "$@"
            [ -n "$(_clone_opt dest)" ] || _clone_die usage "unknown 'k2 clone $sub' (k2 clone --help); a bare workspace needs --dest" 2
            [ "${#_CLONE_ARGS[@]}" -eq 1 ] || _clone_die usage "usage: k2 clone <ws> --dest DIR" 2
            _clone_person_guard
            _clone_py here "${_CLONE_ARGS[0]}"
            ;;
    esac
}

_users_usage() {
    cat <<'EOF' >&2
k2 users — local daemon connect-user accounts

USAGE
  k2 users                      List accounts (username, name, role,
                                disabled, PASSWORD, created). PASSWORD is
                                "temporary" while the person still has to
                                choose their own, else "-"
  k2 users list [--json]        Same as above; --json adds firstName,
                                lastName, displayName (names are display
                                only — commands take the username) and
                                mustChangePassword (true = temporary)
  k2 users add <username> [--role owner|admin|member|voice_chat] [--password-stdin] [--keep-password]
                [--share-agent <agent>]... [--share-garden <garden>]... [--share-source <name>]...
                [--fixed-password|--no-fixed-password] [--force]
                                Create an account; prompts for password
                                (hidden) + confirm. The password is
                                TEMPORARY: at their first sign-in K2 asks
                                the person to choose their own. Making a
                                login for yourself? Add --keep-password
                                (K2 can't tell it's yours yet). Default
                                role is member. owner/admin are set right
                                after the account is made; member and
                                voice_chat are made at that role. A
                                member is a trusted operator.
                                voice_chat ("Voice & Chat only") can only
                                join this server's voice rooms, plus what
                                you share with it: --share-agent (their
                                own conversation with that agent; repeat
                                for more) and
                                --share-garden (a Garden, read-only on a
                                phone; id or name) and --share-source (a
                                data source such a Garden may show).
                                Nothing else: no terminals, agents, files,
                                settings, vault. Share ONLY a dedicated
                                demo agent: K2 refuses one with
                                connections, vault grants, mail, compute,
                                no permission prompts or a switch that
                                reaches past its Thread unless --force.
                                Each person gets their own conversation
                                with it, delivered as a guest, but the
                                agent's memory is shared by every login
                                and its own Thread: never use that
                                agent's Thread yourself, and give each
                                reviewer their own login. A voice_chat
                                password is fixed and kept by default
                                (they can't change it; a wrong guess
                                locks only their own address):
                                --no-fixed-password lets them change it.
                                For other limited access use an app pass
                                (k2 app).
  k2 users set-role <username> owner|admin|member|voice_chat [--fixed-password|--no-fixed-password]
                                Change a login's role. Owner: any change.
                                Admin: only member <-> voice_chat. A
                                change into or out of voice_chat applies
                                at once with no sign-out; others sign the
                                person out. Moving someone to voice_chat
                                ends their paired phones.
  k2 users share <username> [--agent <agent>]... [--garden <garden>]... [--source <name>]... [--force] [--clear] [--json]
                                Set what a voice_chat login can use (it
                                REPLACES the list): agents by address (a
                                dedicated demo agent only; --force to
                                share another, with warnings), Gardens by
                                id or name, data sources by name. No
                                flags: show it. Admin or owner.
  k2 users set-password <username> [--password-stdin] [--keep-password]
                                Reset someone's password (owner only);
                                prompts like add. Signs them out
                                everywhere. TEMPORARY like add, except on
                                a login linked to you as the owner
                                (Settings → General → Your account), which
                                keeps it with no flag. --keep-password:
                                the person keeps the password you typed.
  k2 users audit [--tail N] [--json]
                                Last N auth-audit records (login /
                                change-password / logout / set-password /
                                add-user:
                                outcome, ingress, ip, client). Owner only.
                                Default 50, max 1000. ~/.k2/auth-audit.jsonl
  k2 users set-can-clone <username> on|off
                                Settings → Admin Access → "Can clone" (owner
                                only). Owners can always clone.
  k2 users unlock <username> [--json]
                                Clear a box login lock without changing
                                the password. Exit 0 when no lock is there.
  k2 users pair-phone [--user <login>] [--json] [--invert]
                                Make a one-time code (10 minutes) and QR that
                                pair the K2 Companion phone app. The owner
                                token pairs as --user, else as the login
                                linked to the owner (Settings → General →
                                Your account); a login pairs as itself and
                                types its password again. --invert for
                                light-background terminals. For a custom
                                App's own phone app (its users, its QR on
                                its web page): k2 app pairing --help.
  k2 users phones [--json]      Paired phones (the owner sees everyone's)
                                and the login a new code pairs as.
  k2 users unpair-phone <id> [--json]
                                End one paired phone (ids: k2 users phones).
                                A paired phone also ends after 30 days
                                unused or 180 days in all, on a password
                                change, or when it signs itself out. A
                                desktop logout does not end it.

FLAGS
  --role <r>                    owner | admin | member | voice_chat
  --share-agent / --agent <a>   an agent address (repeat for more)
  --share-garden / --garden <g> a Garden id or name (repeat for more)
  --share-source / --source <s> a data source name a shared Garden may show
  --fixed-password              add / set-role: the person can't change it
                                (the default for an add --role voice_chat)
  --no-fixed-password           add / set-role: the person may change it
  --force                       share an agent that isn't a dedicated demo agent
  --password-stdin              Read password from stdin (automation; no echo)
  --keep-password               add / set-password: not temporary (no
                                forced change at the next sign-in)
  --tail <N>                    audit: how many records (1..1000, default 50)
  --json                        list: {ok,users[]} with names; unlock: daemon body; audit: raw JSON
  --help, -h                    This help

SECURITY
  Password is NEVER accepted on the command line (no --password flag).
  First owner is bootstrapped via the on-box daemon/owner token:
  k2 users add <you> --role owner --keep-password

EXIT CODES
  0 ok · 1 error · 2 bad usage
EOF
}

# Read a password without echo. Interactive: getpass + confirm.
# Automation: --password-stdin (single line from stdin).
# Echoes the password on stdout (caller captures into a local var);
# never logs it. Empty password → non-zero.
_users_read_password() {
    local from_stdin="${1:-0}"
    if [ "$from_stdin" = "1" ]; then
        python3 -c '
import sys
pw = sys.stdin.readline()
if pw.endswith("\n"):
    pw = pw[:-1]
if pw.endswith("\r"):
    pw = pw[:-1]
if not pw:
    sys.stderr.write("Error: empty password on stdin.\n")
    sys.exit(1)
sys.stdout.write(pw)
'
        return $?
    fi
    python3 -c '
import getpass, sys
try:
    a = getpass.getpass("Password: ")
    b = getpass.getpass("Confirm password: ")
except (EOFError, KeyboardInterrupt):
    sys.stderr.write("\nError: password prompt cancelled.\n")
    sys.exit(1)
if not a:
    sys.stderr.write("Error: password must not be empty.\n")
    sys.exit(1)
if a != b:
    sys.stderr.write("Error: passwords do not match.\n")
    sys.exit(1)
sys.stdout.write(a)
'
}

cmd_users() {
    local sub="${1:-list}"
    case "$sub" in
        --help|-h|help)
            _users_usage
            return 0
            ;;
        list|"")
            # Accept bare `k2 users` and `k2 users list`.
            if [ "$sub" = "list" ]; then shift || true; fi
            local as_json=0
            while [ $# -gt 0 ]; do
                case "$1" in
                    --help|-h) _users_usage; return 0 ;;
                    --json) as_json=1; shift ;;
                    *) echo "Usage: k2 users list [--json]" >&2; return 2 ;;
                esac
            done
            local raw
            raw=$(cli_send cli_request "/cli/users") || raw=""
            local users_agent=0
            { _cli_caller_is_agent_passport || [ -n "${K2_HOOK_TOKEN:-${K2SO_HOOK_TOKEN:-}}" ]; } && users_agent=1
            printf '%s' "$raw" | AS_JSON="$as_json" USERS_AGENT="$users_agent" python3 -c '
import json, os, sys
raw = sys.stdin.read()
try:
    d = json.loads(raw) if raw.strip() else {}
except Exception:
    sys.stderr.write("Error: daemon returned an unparseable response: %r\n" % (raw[:200],))
    sys.exit(1)
if not isinstance(d, dict):
    sys.stderr.write("Error: unexpected response from /cli/users\n"); sys.exit(1)
err = d.get("error")
if err is not None:
    text = (err.get("hint") or err.get("code") or str(err)) if isinstance(err, dict) else str(err)
    code = err.get("code", "") if isinstance(err, dict) else ""
    low = (code + " " + text).lower()
    refused = any(w in low for w in ("owner", "admin", "forbidden", "token", "not allowed", "role_required", "gated"))
    # 0.46.1: an agent passport is refused here (people only); say where
    # agents look instead. The daemon refusal itself is unchanged.
    if refused and os.environ.get("USERS_AGENT") == "1":
        sys.stderr.write("Error: owner_only: Owner/admin only; agents use `k2 connections list --users`. (daemon: %s)\n" % text)
        sys.exit(3)
    sys.stderr.write("Error: %s\n" % text)
    sys.exit(3 if refused else 1)
users = d.get("users") or []
if os.environ.get("AS_JSON") == "1":
    # 0.46.0: names ride along (firstName/lastName null when unset;
    # displayName = "First Last", else the username). The username is
    # the identity for every other command.
    out = []
    for u in users:
        if not isinstance(u, dict):
            continue
        username = str(u.get("username") or "")
        out.append({
            "username": username,
            "role": u.get("role") or "member",
            "disabled": bool(u.get("disabled")),
            "wasViewer": bool(u.get("wasViewer")),
            "canClone": u.get("canClone"),
            "createdAt": u.get("createdAt") or u.get("created_at"),
            "firstName": u.get("firstName"),
            "lastName": u.get("lastName"),
            "displayName": u.get("displayName") or username,
            # 0.46.8: true while the password is temporary.
            "mustChangePassword": u.get("mustChangePassword") is True,
            # 0.46.10: the shares of a voice_chat login (empty lists otherwise).
            "shares": u.get("shares") or {"agents": [], "gardens": []},
        })
    doc = {"ok": True, "users": out}
    if d.get("doctor"):
        doc["doctor"] = d["doctor"]
    print(json.dumps(doc, ensure_ascii=False, separators=(",", ":")))
    sys.exit(0)
if not users:
    print("No connect users yet.")
    print("Create the first owner: k2 users add <username> --role owner --keep-password")
    sys.exit(0)
# Table: USERNAME  NAME  ROLE  DISABLED  PASSWORD  CREATED
rows = []
for u in users:
    if not isinstance(u, dict):
        continue
    disabled = "yes" if u.get("disabled") else "no"
    if u.get("wasViewer"):
        # Viewer role removed: disabled on the update until the owner
        # enables it as a member (Settings, or POST users/set-disabled).
        disabled += " (was viewer)"
    # 0.46.0: the real name when one is set (display only), else "-".
    has_name = bool(u.get("firstName") or u.get("lastName"))
    name = str(u.get("displayName") or "") if has_name else ""
    rows.append((
        str(u.get("username") or "?"),
        name or "-",
        str(u.get("role") or "member"),
        disabled,
        # 0.46.8: "temporary" until the person chooses their own.
        "temporary" if u.get("mustChangePassword") is True else "-",
        str(u.get("createdAt") or u.get("created_at") or "-"),
    ))
if not rows:
    print("No connect users yet.")
    sys.exit(0)
w_user = max([len(r[0]) for r in rows] + [len("USERNAME")])
w_name = max([len(r[1]) for r in rows] + [len("NAME")])
w_role = max([len(r[2]) for r in rows] + [len("ROLE")])
w_dis = max([len(r[3]) for r in rows] + [len("DISABLED")])
w_pw = max([len(r[4]) for r in rows] + [len("PASSWORD")])
print("%-*s  %-*s  %-*s  %-*s  %-*s  %s" % (w_user, "USERNAME", w_name, "NAME", w_role, "ROLE", w_dis, "DISABLED", w_pw, "PASSWORD", "CREATED"))
for u, name, role, dis, pw, created in rows:
    print("%-*s  %-*s  %-*s  %-*s  %-*s  %s" % (w_user, u, w_name, name, w_role, role, w_dis, dis, w_pw, pw, created))
# 0.46.10: what each Voice & Chat only login can use.
for u in users:
    if isinstance(u, dict) and u.get("role") == "voice_chat":
        sh = u.get("shares") or {}
        ag = ", ".join(str(a.get("address") or (a.get("id", "?") + " (gone)")) for a in sh.get("agents") or []) or "none"
        gd = ", ".join(str(g.get("name") or (g.get("id", "?") + " (gone)")) for g in sh.get("gardens") or []) or "none"
        print("  %s (Voice & Chat only) shares: agents %s; gardens %s" % (u.get("username"), ag, gd))
# 0.46.6: a login with a name K2 keeps for itself acts as nobody.
for line in d.get("doctor") or []:
    sys.stderr.write("doctor: %s\n" % line)
'
            ;;
        add)
            shift || true
            local username="" role="" password_stdin=0 keep_password=0
            local share_agents="" share_gardens="" share_sources="" fixed_password="" force=0
            while [ $# -gt 0 ]; do
                case "$1" in
                    --fixed-password)
                        fixed_password=1; shift ;;
                    --no-fixed-password)
                        fixed_password=0; shift ;;
                    --force)
                        force=1; shift ;;
                    --share-source)
                        [ -n "${2:-}" ] || { echo "Error: --share-source needs a data source name" >&2; return 2; }
                        share_sources="${share_sources}${2}"$'\n'; shift 2 ;;
                    --role)
                        role="${2:-}"; shift 2 || { echo "Error: --role needs a value" >&2; return 2; } ;;
                    --role=*)
                        role="${1#--role=}"; shift ;;
                    --share-agent)
                        [ -n "${2:-}" ] || { echo "Error: --share-agent needs an agent address" >&2; return 2; }
                        share_agents="${share_agents}${2}"$'\n'; shift 2 ;;
                    --share-agent=*)
                        share_agents="${share_agents}${1#--share-agent=}"$'\n'; shift ;;
                    --share-garden)
                        [ -n "${2:-}" ] || { echo "Error: --share-garden needs a Garden id or name" >&2; return 2; }
                        share_gardens="${share_gardens}${2}"$'\n'; shift 2 ;;
                    --share-garden=*)
                        share_gardens="${share_gardens}${1#--share-garden=}"$'\n'; shift ;;
                    --password-stdin)
                        password_stdin=1; shift ;;
                    --keep-password)
                        keep_password=1; shift ;;
                    --password|--pass)
                        echo "Error: passwords must not be passed on the command line." >&2
                        echo "Use an interactive prompt or --password-stdin." >&2
                        return 2
                        ;;
                    --help|-h)
                        _users_usage; return 0 ;;
                    -*)
                        echo "Error: unknown flag '$1'" >&2
                        _users_usage
                        return 2
                        ;;
                    *)
                        if [ -z "$username" ]; then
                            username="$1"; shift
                        else
                            echo "Error: unexpected argument '$1'" >&2
                            return 2
                        fi
                        ;;
                esac
            done
            if [ -z "$username" ]; then
                echo "Usage: k2 users add <username> [--role owner|admin|member|voice_chat] [--share-agent <agent>]... [--share-garden <garden>]... [--password-stdin] [--keep-password]" >&2
                return 2
            fi
            if [ -n "$role" ]; then
                case "$role" in
                    owner|admin|member) ;;
                    voice_chat|voice-chat) role="voice_chat" ;;
                    viewer|Viewer|VIEWER)
                        # Same text the daemon returns (k2_core VIEWER_ROLE_REMOVED).
                        echo "Error: the viewer role was removed: a login is owner, admin or member, or Voice & Chat only (voice_chat) for someone who should only use voice rooms. For other limited access, give the person an app pass instead (k2 app)." >&2
                        return 2
                        ;;
                    *)
                        echo "Error: invalid role '$role' (expected owner|admin|member|voice_chat)" >&2
                        return 2
                        ;;
                esac
            fi
            if { [ -n "$share_agents" ] || [ -n "$share_gardens" ] || [ -n "$share_sources" ]; } && [ "$role" != "voice_chat" ]; then
                echo "Error: --share-agent, --share-garden and --share-source are only for --role voice_chat" >&2
                return 2
            fi
            local password
            password="$(_users_read_password "$password_stdin")" || return 1
            local body resp
            # 0.46.8: temporary unless --keep-password (sent either way, so
            # an older daemon that defaulted to false does the same).
            # member / voice_chat ride the add body (made at that role in one
            # write); owner/admin keep the set-role call below.
            local body_role=""
            case "$role" in member|voice_chat) body_role="$role" ;; esac
            body=$(USERNAME="$username" PASSWORD="$password" KEEP="$keep_password" ROLE="$body_role" \
                SHARE_AGENTS="$share_agents" SHARE_GARDENS="$share_gardens" SHARE_SOURCES="$share_sources" \
                FIXED="$fixed_password" FORCE="$force" python3 -c '
import json, os
b = {"username": os.environ["USERNAME"], "password": os.environ["PASSWORD"]}
if os.environ["ROLE"] == "voice_chat":
    # L4: a voice_chat login is fixed and kept unless a flag says otherwise
    # (the daemon decides; only flags given are sent).
    if os.environ["KEEP"] == "1":
        b["mustChangePassword"] = False
else:
    b["mustChangePassword"] = os.environ["KEEP"] != "1" and os.environ["FIXED"] != "1"
if os.environ["ROLE"]:
    b["role"] = os.environ["ROLE"]
agents = [a for a in os.environ["SHARE_AGENTS"].split("\n") if a.strip()]
gardens = [g for g in os.environ["SHARE_GARDENS"].split("\n") if g.strip()]
sources = [s for s in os.environ["SHARE_SOURCES"].split("\n") if s.strip()]
if agents:
    b["shareAgents"] = agents
if gardens:
    b["shareGardens"] = gardens
if sources:
    b["shareSources"] = sources
if os.environ["FIXED"] == "1":
    b["fixedPassword"] = True
elif os.environ["FIXED"] == "0":
    b["fixedPassword"] = False
if os.environ["FORCE"] == "1":
    b["force"] = True
print(json.dumps(b))
')
            # Drop password from the environment as soon as the body is built.
            unset password
            resp=$(cli_send cli_post_json "/cli/users/add" "$body") || resp=""
            unset body
            printf '%s' "$resp" | USERNAME="$username" KEEP="$keep_password" python3 -c '
import json, os, sys
raw = sys.stdin.read()
username = os.environ.get("USERNAME", "?")
try:
    d = json.loads(raw) if raw.strip() else {}
except Exception:
    sys.stderr.write("Error: daemon returned an unparseable response: %r\n" % (raw[:200],))
    sys.exit(1)
err = d.get("error") if isinstance(d, dict) else None
if err is not None:
    hint = (err.get("hint") or err.get("message") or err.get("code")) if isinstance(err, dict) else (d.get("message") or err)
    sys.stderr.write("Error: %s\n" % hint)
    for r in (err.get("risks") or []) if isinstance(err, dict) else []:
        sys.stderr.write("  - %s\n" % r)
    sys.exit(1)
if not (isinstance(d, dict) and d.get("success")):
    sys.stderr.write("Error: users/add failed: %s\n" % (raw[:200] or "empty response"))
    sys.exit(1)
print("Created user %s." % username)
if d.get("role") == "voice_chat":
    sh = d.get("shares") or {}
    ag = ", ".join(str(a.get("address") or a.get("id")) for a in sh.get("agents") or []) or "none"
    gd = ", ".join(str(g.get("name") or g.get("id")) for g in sh.get("gardens") or []) or "none"
    print("Voice & Chat only: joins voice rooms; shared agents: %s; shared Gardens: %s." % (ag, gd))
    for w in d.get("warnings") or []:
        sys.stderr.write("WARNING: shared anyway (--force): %s\n" % w)
    if d.get("notice"):
        sys.stderr.write("WARNING: %s\n" % d["notice"])
if d.get("fixedPassword") is True:
    print("The password is fixed: the person cannot change it.")
temp = d.get("mustChangePassword")
if temp is None:
    temp = os.environ.get("KEEP") != "1"
if temp is True:
    print("The password is temporary: the first time %s signs in, K2 asks them to choose their own." % username)
else:
    print("The password is kept (not temporary).")
'
            # owner/admin need the explicit write after the add (member and
            # voice_chat were made at their role by the add itself).
            if [ "$role" = "owner" ] || [ "$role" = "admin" ]; then
                local role_body role_resp
                role_body=$(USERNAME="$username" ROLE="$role" python3 -c '
import json, os
print(json.dumps({"username": os.environ["USERNAME"], "role": os.environ["ROLE"]}))
')
                role_resp=$(cli_send cli_post_json "/cli/users/set-role" "$role_body") || role_resp=""
                printf '%s' "$role_resp" | USERNAME="$username" ROLE="$role" python3 -c '
import json, os, sys
raw = sys.stdin.read()
try:
    d = json.loads(raw) if raw.strip() else {}
except Exception:
    sys.stderr.write("Error: set-role returned unparseable response: %r\n" % (raw[:200],))
    sys.exit(1)
err = d.get("error") if isinstance(d, dict) else None
if err is not None:
    hint = err.get("hint") if isinstance(err, dict) else err
    sys.stderr.write("Error setting role: %s\n" % hint)
    sys.exit(1)
if not (isinstance(d, dict) and d.get("success")):
    sys.stderr.write("Error: set-role failed: %s\n" % (raw[:200] or "empty response"))
    sys.exit(1)
print("Role set to %s for %s." % (os.environ.get("ROLE", "?"), os.environ.get("USERNAME", "?")))
'
            fi
            ;;
        set-role)
            # 0.46.10: POST /cli/users/set-role. Owner: any change; Admin:
            # member <-> voice_chat only (the daemon decides).
            shift || true
            local sr_user="" sr_role="" sr_fixed=""
            while [ $# -gt 0 ]; do
                case "$1" in
                    --help|-h) _users_usage; return 0 ;;
                    --fixed-password) sr_fixed="true"; shift ;;
                    --no-fixed-password) sr_fixed="false"; shift ;;
                    -*) echo "Error: unknown flag '$1'" >&2; return 2 ;;
                    *)
                        if [ -z "$sr_user" ]; then sr_user="$1"
                        elif [ -z "$sr_role" ]; then sr_role="$1"
                        else echo "Error: unexpected argument '$1'" >&2; return 2; fi
                        shift ;;
                esac
            done
            if [ -z "$sr_user" ] || [ -z "$sr_role" ]; then
                echo "Usage: k2 users set-role <username> owner|admin|member|voice_chat [--fixed-password|--no-fixed-password]" >&2
                return 2
            fi
            case "$sr_role" in
                owner|admin|member|voice_chat) ;;
                voice-chat) sr_role="voice_chat" ;;
                *) echo "Error: invalid role '$sr_role' (expected owner|admin|member|voice_chat)" >&2; return 2 ;;
            esac
            local sr_body sr_resp
            sr_body=$(SR_USER="$sr_user" SR_ROLE="$sr_role" SR_FIXED="$sr_fixed" python3 -c '
import json, os
b = {"username": os.environ["SR_USER"], "role": os.environ["SR_ROLE"]}
if os.environ["SR_FIXED"]:
    b["fixedPassword"] = os.environ["SR_FIXED"] == "true"
print(json.dumps(b))')
            sr_resp=$(cli_send cli_post_json "/cli/users/set-role" "$sr_body") || sr_resp=""
            printf '%s' "$sr_resp" | SR_USER="$sr_user" SR_ROLE="$sr_role" python3 -c '
import json, os, sys
raw = sys.stdin.read()
try:
    d = json.loads(raw) if raw.strip() else {}
except Exception:
    sys.stderr.write("Error: daemon returned an unparseable response: %r\n" % (raw[:200],)); sys.exit(1)
if isinstance(d, dict) and d.get("success"):
    live = d.get("live")
    print("%s: %s -> %s%s" % (os.environ["SR_USER"], d.get("from", "?"), d.get("role", os.environ["SR_ROLE"]),
          " (applied now, no sign-out)" if live else " (they sign in again)" if live is False else ""))
    sys.exit(0)
e = d.get("error") if isinstance(d, dict) else None
msg = (d.get("message") if isinstance(d, dict) else None) or ((e.get("hint") or e.get("code")) if isinstance(e, dict) else e) or (raw[:200] or "empty response from daemon")
sys.stderr.write("Error: %s\n" % msg)
code = e if isinstance(e, str) else ""
sys.exit(3 if code in ("role_change_not_allowed", "role_required", "invalid or missing token") else 1)'
            return $?
            ;;
        share|shares)
            # 0.46.10: a voice_chat login's shares. POST /cli/users/set-shares
            # replaces both lists; no flags = show (GET /cli/users).
            shift || true
            local sh_user="" sh_agents="" sh_gardens="" sh_sources="" sh_set=0 sh_json=0 sh_force=0
            while [ $# -gt 0 ]; do
                case "$1" in
                    --source|--share-source)
                        [ -n "${2:-}" ] || { echo "Error: $1 needs a data source name" >&2; return 2; }
                        sh_sources="${sh_sources}${2}"$'\n'; sh_set=1; shift 2 ;;
                    --force) sh_force=1; shift ;;
                    --agent|--share-agent)
                        [ -n "${2:-}" ] || { echo "Error: $1 needs an agent address" >&2; return 2; }
                        sh_agents="${sh_agents}${2}"$'\n'; sh_set=1; shift 2 ;;
                    --garden|--share-garden)
                        [ -n "${2:-}" ] || { echo "Error: $1 needs a Garden id or name" >&2; return 2; }
                        sh_gardens="${sh_gardens}${2}"$'\n'; sh_set=1; shift 2 ;;
                    --clear) sh_set=1; shift ;;
                    --json) sh_json=1; shift ;;
                    --help|-h) _users_usage; return 0 ;;
                    -*) echo "Error: unknown flag '$1'" >&2; return 2 ;;
                    *)
                        if [ -z "$sh_user" ]; then sh_user="$1"; shift
                        else echo "Error: unexpected argument '$1'" >&2; return 2; fi ;;
                esac
            done
            if [ -z "$sh_user" ]; then
                echo "Usage: k2 users share <username> [--agent <agent>]... [--garden <garden>]... [--source <name>]... [--force] [--clear] [--json]" >&2
                return 2
            fi
            local sh_resp
            if [ "$sh_set" = "1" ]; then
                local sh_body
                sh_body=$(SH_USER="$sh_user" SH_AGENTS="$sh_agents" SH_GARDENS="$sh_gardens" SH_SOURCES="$sh_sources" SH_FORCE="$sh_force" python3 -c '
import json, os
b = {"username": os.environ["SH_USER"],
     "agents": [a for a in os.environ["SH_AGENTS"].split("\n") if a.strip()],
     "gardens": [g for g in os.environ["SH_GARDENS"].split("\n") if g.strip()]}
sources = [s for s in os.environ["SH_SOURCES"].split("\n") if s.strip()]
if sources:
    b["sources"] = sources
if os.environ["SH_FORCE"] == "1":
    b["force"] = True
print(json.dumps(b))')
                sh_resp=$(cli_send cli_post_json "/cli/users/set-shares" "$sh_body") || sh_resp=""
            else
                sh_resp=$(cli_send cli_request "/cli/users") || sh_resp=""
            fi
            printf '%s' "$sh_resp" | SH_USER="$sh_user" SH_JSON="$sh_json" python3 -c '
import json, os, sys
raw = sys.stdin.read()
user = os.environ["SH_USER"].strip().lower()
try:
    d = json.loads(raw) if raw.strip() else {}
except Exception:
    sys.stderr.write("Error: daemon returned an unparseable response: %r\n" % (raw[:200],)); sys.exit(1)
e = d.get("error") if isinstance(d, dict) else None
if e is not None:
    msg = d.get("message") or ((e.get("hint") or e.get("code")) if isinstance(e, dict) else e)
    sys.stderr.write("Error: %s\n" % msg)
    for r in (e.get("risks") or []) if isinstance(e, dict) else []:
        sys.stderr.write("  - %s\n" % r)
    code = e.get("code") if isinstance(e, dict) else e
    sys.exit(3 if code in ("role_required", "invalid or missing token", "Invalid or missing auth token") else 1)
for w in d.get("warnings") or []:
    sys.stderr.write("WARNING: shared anyway (--force): %s\n" % w)
if d.get("notice"):
    sys.stderr.write("WARNING: %s\n" % d["notice"])
if "users" in d:
    row = next((u for u in d.get("users") or [] if isinstance(u, dict) and u.get("username") == user), None)
    if row is None:
        sys.stderr.write("Error: no login %s\n" % user); sys.exit(1)
    if row.get("role") != "voice_chat":
        sys.stderr.write("Error: %s is %s; shares are only for a voice_chat login\n" % (user, row.get("role"))); sys.exit(1)
    shares = row.get("shares") or {"agents": [], "gardens": []}
else:
    shares = d.get("shares") or {"agents": [], "gardens": []}
if os.environ["SH_JSON"] == "1":
    print(json.dumps({"ok": True, "username": user, "shares": shares}, separators=(",", ":")))
    sys.exit(0)
ag = ", ".join(str(a.get("address") or (a.get("id", "?") + " (gone)")) for a in shares.get("agents") or []) or "none"
gd = ", ".join(str(g.get("name") or (g.get("id", "?") + " (gone)")) for g in shares.get("gardens") or []) or "none"
sr = ", ".join(str(s.get("name", "?")) + (" (gone)" if s.get("missing") else "") for s in shares.get("sources") or []) or "none"
print("%s (Voice & Chat only)\n  agents:  %s\n  gardens: %s\n  sources: %s" % (user, ag, gd, sr))'
            return $?
            ;;
        set-password)
            # 0.46.8: owner reset of someone's password (POST
            # /cli/users/set-password; owner token or an Owner login).
            # Temporary unless --keep-password; the daemon keeps a reset
            # of the owner's own linked login as theirs.
            shift || true
            local username="" password_stdin=0 keep_password=0
            while [ $# -gt 0 ]; do
                case "$1" in
                    --password-stdin) password_stdin=1; shift ;;
                    --keep-password) keep_password=1; shift ;;
                    --password|--pass)
                        echo "Error: passwords must not be passed on the command line." >&2
                        echo "Use an interactive prompt or --password-stdin." >&2
                        return 2
                        ;;
                    --help|-h) _users_usage; return 0 ;;
                    -*)
                        echo "Error: unknown flag '$1'" >&2
                        _users_usage
                        return 2
                        ;;
                    *)
                        if [ -z "$username" ]; then
                            username="$1"; shift
                        else
                            echo "Error: unexpected argument '$1'" >&2
                            return 2
                        fi
                        ;;
                esac
            done
            if [ -z "$username" ]; then
                echo "Usage: k2 users set-password <username> [--password-stdin] [--keep-password]" >&2
                return 2
            fi
            local password
            password="$(_users_read_password "$password_stdin")" || return 1
            local body resp
            body=$(USERNAME="$username" PASSWORD="$password" KEEP="$keep_password" python3 -c '
import json, os
b = {"username": os.environ["USERNAME"], "password": os.environ["PASSWORD"]}
if os.environ["KEEP"] == "1":
    b["mustChangePassword"] = False
print(json.dumps(b))
')
            unset password
            resp=$(cli_send cli_post_json "/cli/users/set-password" "$body") || resp=""
            unset body
            printf '%s' "$resp" | USERNAME="$username" python3 -c '
import json, os, sys
raw = sys.stdin.read()
username = os.environ.get("USERNAME", "?")
try:
    d = json.loads(raw) if raw.strip() else {}
except Exception:
    sys.stderr.write("Error: daemon returned an unparseable response: %r\n" % (raw[:200],))
    sys.exit(1)
err = d.get("error") if isinstance(d, dict) else None
if err is not None:
    hint = err.get("hint") if isinstance(err, dict) else err
    sys.stderr.write("Error: %s\n" % hint)
    low = str(hint).lower()
    sys.exit(3 if ("token" in low or "forbidden" in low or "may not" in low) else 1)
if not (isinstance(d, dict) and d.get("success")):
    sys.stderr.write("Error: set-password failed: %s\n" % (raw[:200] or "empty response"))
    sys.exit(1)
print("Password set for %s. They are signed out everywhere." % username)
if d.get("mustChangePassword") is True:
    print("It is temporary: the next time %s signs in, K2 asks them to choose their own." % username)
else:
    print("The password is kept (not temporary).")
'
            ;;
        audit)
            # PRD connect-login-edge-only L3 — GET /cli/users/audit?tail=N
            # (owner token or Owner-role session; the daemon 405s POST).
            shift || true
            local tail_n="" as_json=0
            while [ $# -gt 0 ]; do
                case "$1" in
                    --tail)
                        tail_n="${2:-}"; shift 2 || { echo "Error: --tail needs a value" >&2; return 2; } ;;
                    --tail=*)
                        tail_n="${1#--tail=}"; shift ;;
                    --json)
                        as_json=1; shift ;;
                    --help|-h)
                        _users_usage; return 0 ;;
                    *)
                        echo "Error: unexpected argument '$1'" >&2
                        echo "Usage: k2 users audit [--tail N] [--json]" >&2
                        return 2
                        ;;
                esac
            done
            if [ -n "$tail_n" ] && ! printf '%s' "$tail_n" | grep -Eq '^[0-9]+$'; then
                echo "Error: --tail must be an integer 1..1000 (got '$tail_n')" >&2
                return 2
            fi
            local raw
            if [ -n "$tail_n" ]; then
                raw=$(cli_send cli_request "/cli/users/audit" "tail=${tail_n}") || raw=""
            else
                raw=$(cli_send cli_request "/cli/users/audit") || raw=""
            fi
            if [ "$as_json" = "1" ]; then printf '%s\n' "$raw"; return 0; fi
            printf '%s' "$raw" | python3 -c '
import json, sys
raw = sys.stdin.read()
try:
    d = json.loads(raw) if raw.strip() else {}
except Exception:
    sys.stderr.write("Error: daemon returned an unparseable response: %r\n" % (raw[:200],))
    sys.exit(1)
if not isinstance(d, dict):
    sys.stderr.write("Error: unexpected response from /cli/users/audit\n"); sys.exit(1)
err = d.get("error")
if err is not None:
    hint = err.get("hint") if isinstance(err, dict) else err
    sys.stderr.write("Error: %s\n" % hint)
    sys.exit(1)
events = d.get("events") or []
if not events:
    print("No auth-audit records yet (%s)." % (d.get("path") or "~/.k2/auth-audit.jsonl"))
    sys.exit(0)
rows = []
for e in events:
    if not isinstance(e, dict):
        continue
    rows.append((
        str(e.get("ts") or "-"),
        str(e.get("event") or "-"),
        str(e.get("outcome") or "-"),
        str(e.get("user") or "-"),
        str(e.get("ingress") or "-"),
        str(e.get("ip") or "-"),
        str(e.get("client") or "-"),
    ))
hdr = ("TS", "EVENT", "OUTCOME", "USER", "INGRESS", "IP", "CLIENT")
widths = [max(len(r[i]) for r in rows + [hdr]) for i in range(len(hdr))]
def line(r):
    return "  ".join(r[i].ljust(widths[i]) for i in range(len(hdr))).rstrip()
print(line(hdr))
for r in rows:
    print(line(r))
'
            ;;
        set-can-clone)
            # Settings → Admin Access → "Can clone" (owner only).
            shift || true
            local cc_user="${1:-}" cc_val="${2:-}"
            case "$cc_val" in
                on|true|yes|1) cc_val=true ;;
                off|false|no|0) cc_val=false ;;
                *) echo "Usage: k2 users set-can-clone <username> on|off" >&2; return 2 ;;
            esac
            [ -n "$cc_user" ] || { echo "Usage: k2 users set-can-clone <username> on|off" >&2; return 2; }
            local cc_body cc_resp
            cc_body=$(CC_USER="$cc_user" CC_VAL="$cc_val" python3 -c '
import json, os
print(json.dumps({"username": os.environ["CC_USER"], "canClone": os.environ["CC_VAL"] == "true"}))')
            cc_resp=$(cli_send cli_post_json "/cli/users/set-can-clone" "$cc_body") || cc_resp=""
            printf '%s' "$cc_resp" | CC_USER="$cc_user" CC_VAL="$cc_val" python3 -c '
import json, os, sys
raw = sys.stdin.read()
try:
    d = json.loads(raw) if raw.strip() else {}
except Exception:
    d = {"error": raw[:200]}
if d.get("success"):
    print("%s: Can clone %s" % (os.environ["CC_USER"], "on" if os.environ["CC_VAL"] == "true" else "off"))
    sys.exit(0)
e = d.get("error") or "empty response from daemon"
print("Error: %s" % (e.get("hint") if isinstance(e, dict) else e), file=sys.stderr)
sys.exit(3)'
            return $?
            ;;
        unlock)
            shift || true
            local username="" as_json=0
            while [ $# -gt 0 ]; do
                case "$1" in
                    --json) as_json=1; shift ;;
                    --help|-h) _users_usage; return 0 ;;
                    -*)
                        echo "Error: unknown flag '$1'" >&2
                        _users_usage
                        return 2
                        ;;
                    *)
                        if [ -z "$username" ]; then
                            username="$1"; shift
                        else
                            echo "Error: unexpected argument '$1'" >&2
                            return 2
                        fi
                        ;;
                esac
            done
            if [ -z "$username" ]; then
                echo "Usage: k2 users unlock <username> [--json]" >&2
                return 2
            fi
            local body resp
            body=$(USERNAME="$username" python3 -c '
import json, os
print(json.dumps({"username": os.environ["USERNAME"]}))
')
            resp=$(cli_send cli_post_json "/cli/users/unlock" "$body") || resp=""
            printf '%s' "$resp" | AS_JSON="$as_json" python3 -c '
import json, os, sys
raw = sys.stdin.read()
as_json = os.environ.get("AS_JSON") == "1"
if not raw.strip():
    sys.stderr.write("Error: empty response from daemon (is it running?)\n")
    sys.exit(1)
try:
    d = json.loads(raw)
except Exception:
    sys.stderr.write("Error: daemon returned an unparseable response: %r\n" % (raw[:200],))
    sys.exit(1)
if not isinstance(d, dict):
    sys.stderr.write("Error: unexpected response from /cli/users/unlock\n")
    sys.exit(1)
err = d.get("error")
if err is None and d.get("ok") is False:
    err = d
if err is not None:
    if isinstance(err, dict):
        hint = err.get("hint") or err.get("message") or err.get("code") or "error"
    else:
        hint = err
    sys.stderr.write("Error: %s\n" % hint)
    sys.exit(1)
if d.get("ok") is not True:
    sys.stderr.write("Error: %s\n" % (raw[:200] or "empty response"))
    sys.exit(1)
if as_json:
    sys.stdout.write(raw if raw.endswith("\n") else raw + "\n")
    sys.exit(0)
key = d.get("username") or ""
if d.get("cleared") is True:
    print("Lock cleared for %s" % key)
else:
    print("No lock for %s" % key)
'
            ;;
        pair-phone)
            shift || true
            _users_pair_phone "$@"
            ;;
        phones)
            shift || true
            _users_phones "$@"
            ;;
        unpair-phone)
            shift || true
            _users_unpair_phone "$@"
            ;;
        *)
            echo "Unknown 'k2 users' subcommand: $sub" >&2
            _users_usage
            return 2
            ;;
    esac
}

