# Update-key ceremony (U1 → U2, with an offline backup U3)

Runbook for the planned rotation of K2's update-signing key
(prd-security-sweep-0464-v1, SS4/SS5, option C "dual-sign bridge").
Rosson runs it **once**, on **z3mbpZ**, in a **GUI terminal** (iTerm or
Terminal, not SSH). With the signing user it is four short sittings (about
80 minutes in all, below), and it must be done before
0.46.6 can be released: `release.sh` refuses to start while the new key is
missing.

> **Rosson chose the signing user (2026-10-28):** each signing Mac's primary
> key (U2 on z3mbpZ, U2b on z3mbp) is born inside that Mac's `_k2sign` macOS
> user and never sits in `~/.k2-updater` or the keychain.
> Follow **"The signing user"** right below. The keychain steps further down
> (sections 1–2, "the stopgap") stay only as the rollback path
> (`K2_SIGNER=keychain`) until two releases have signed green through
> `_k2sign`.

## The signing user (prd-signing-user-v1; the default backend)

**What it is.** On each Mac that releases, a hidden macOS role user `_k2sign`
(own group, UID/GID 400–499, shell `/usr/bin/false`, no password, home
`/var/k2sign` 0700) holds that Mac's update keys and their passwords. Rosson's
user can run exactly one thing as it, with no password:

    sudo -n -u _k2sign /usr/local/libexec/k2sign/k2sign <verb>

File bytes go in on stdin and the signature comes out on stdout. The signer
runs only root-owned tools by absolute path (a pinned minisign 0.12, a pinned
gh 2.83.1, `/usr/bin/openssl`, the Command Line Tools python3; never
`/opt/homebrew`, and never Xcode.app's python, because `/Applications` is
admin-writable). It signs only inside a **session** Rosson opened with
**one approval** at `release.sh` Step 0.46 (before any build), bound to the
version, the base commit and the asset list, for 12 hours, and only files
that pass its own checks (below). Every signature is in
`/var/k2sign/audit/<version>/signatures.tsv` (`~/k2-release-audit` gets a
marked copy).

**Two signing Macs, one key each (Rosson 2026-10-28).** Releases can run on
z3mbpZ or on z3mbp (the M5). Each Mac makes its **own** primary key inside its
own `_k2sign`; no key is ever copied between them:

| Key | Mac | Made by | Trusted by 0.46.6+ |
|---|---|---|---|
| **U2** | z3mbpZ | `k2sign-admin generate-u2` on z3mbpZ (sitting 2) | yes (primary) |
| **U2b** | z3mbp | `k2sign-admin generate-u2` on z3mbp (sitting 2 with `--key U2b`) | yes (primary) |
| **U3** | none: the password manager | the ceremony, once, during z3mbpZ's sitting 2 | yes (backup) |
| **U1** | both Macs (imported into each `_k2sign`) | the old key, retired | installs older than 0.46.6 only (the bridge) |

- Installs trust **any** of U2, U2b and U3 (`UPDATER_KEYS`: two primaries,
  then the backup). A release is signed with the key of the Mac it runs on:
  `sign-updater.sh` asks that Mac's signer which primary it holds
  (`k2sign keys`) and checks it is one of the source tree's primaries;
  `latest.json` files the signature under that key's id.
- A lost or stolen Mac is revoked by dropping **its** key alone from
  `UPDATER_KEYS` in the next release (installs that update past it stop
  trusting it); the other Mac keeps releasing. U3 is the backup if both are
  lost.
- Releasing from z3mbp is Rosson's choice when he uses it. A release build is
  heavy on the M5 (cargo, notarization, the DMG): pause the other builders
  first. z3mbp also needs what any release Mac needs: the Developer ID
  identity in its login keychain, the notarization key, a GUI iTerm.
- Each Mac's signer state is its own: a version approved on one Mac is not
  approved on the other.

**Approving: Touch ID or the Mac login password (Rosson 2026-10-28).** The
approver key is a Secure Enclave key whose access control is
`[.privateKeyUsage, .biometryAny, .or, .devicePasscode]`: the system sheet on
that Mac's screen asks for a fingerprint, or "Use Password…" takes the login
password. A fresh sheet every time (nothing is cached or pre-evaluated). The
sheet appears on that Mac's screen even when the app was started over SSH
(that is how `release.sh` works from SSH): the boundary is the person
answering the sheet, never how the app was launched. With nobody at the
screen nothing is approved; the sheet closes itself after `--timeout`
(`sitting.sh reenrol` proof 3 shows this hands-off). Adding or removing a fingerprint no longer
kills the key (`.biometryAny`). What the password adds, written down: anyone
who has the login password and the screen (in person, or Screen Sharing /
RemotePC) can approve. The login password is also the admin password, which
already opens the break-glass `k2sign-admin approve`, so it gives no new
power; keep remote-GUI tools off on both Macs (cleanup items 8–9).

**Sittings** (all in `scripts/k2sign/sitting.sh`). Each prints its exact
commands and changes nothing until you add `--run`; then it stops before
every step (Enter runs it, `s` skips a step already done on an earlier try).
Sittings 1–3 and `reenrol` take `--key U2` (on z3mbpZ) or `--key U2b` (on
z3mbp), refuse to run on the other Mac, and take `--commit <sha>`, the
commit K2 names.

**Before the first sitting on a Mac**
- The release checkout on that Mac is on the commit K2 names:
  `cd ~/DevProjects/Alakazam\ Labs/K2 && git fetch origin && git checkout --detach <sha>`.
  (Back to main afterwards: `git checkout main`.)
- At least 60 GiB free; the Command Line Tools installed (`xcode-select --install`).
- `bash scripts/k2sign/sitting.sh check --key U2` (or `U2b`) says `ready.`

| Sitting | Where | When | Time | You type | You approve |
|---|---|---|---|---|---|
| `check` | each Mac | any time | 1 min | nothing | nothing |
| `0` (S0 spike) | z3mbpZ (z3mbp optional) | first; wait for K2's go | ~10 min | one `ssh` line from another Mac | Touch ID ×4 |
| `1 --key U2` / `--key U2b` | that Mac | after K2's go on sitting 0 | ~25 min | Mac admin password ×2–3, that Mac's new GitHub token; send K2 the hashes | ×1 (enrol) |
| `reenrol` | z3mbpZ only | z3mbpZ's sitting 1 ran before the password change (87b27c2dc) | ~10 min | admin password ×1, the login password once (proof 2), one `ssh` line | Touch ID ×1–2 |
| `2 --key U2` (agents stopped) | z3mbpZ | after its hashes match | ~30 min | admin password ×1–2, U3's new password ×2, `saved`, U1's password ×1; send K2 `~/k2-update-pubkeys-U2.txt` | nothing |
| `2 --key U2b` (agents stopped) | z3mbp | after its hashes match | ~15 min | admin password ×1–2, U1's password ×1; send K2 `~/k2-update-pubkeys-U2b.txt` | nothing |
| `3 --key U2` / `--key U2b` | each Mac | after K2 lands the public keys | ~15 min | admin password ×1; send K2 the hashes again | ×1 |

U1's password must be in your password manager before sitting 2 (today it
is the value on the `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` line in the K2
checkout's `.env`: open the file in an editor, don't `cat` it). After pasting
any password or a token, copy other text so it isn't left on the clipboard.
Sitting 2 on z3mbp stops every agent on z3mbp too, including the K2 workspace
agents that build this code.

**Order against landing.** The lane stays held until the public keys exist,
because it changes what installs trust and `release.sh` refuses to start
without them. **Both Macs' sittings 1 and 2 come before the public keys
land**, so one pubkeys commit carries U2, U2b and U3 and there is only one
updater-key rollout:
1. From the **lane's commit** (K2 pushes it as a branch and names the commit):
   z3mbpZ `0`, `1 --key U2` (done 2026-10-28 at 87b27c2dc), `reenrol --key U2`,
   `2 --key U2`; z3mbp `1 --key U2b`, `2 --key U2b`. The two Macs' sittings
   can interleave; sitting 2 needs agents stopped on that Mac only.
2. K2 runs `bash scripts/updater-key-ceremony.sh pubkeys <U2> <U2b> <U3>`
   (the backup last) on the lane, runs the gates, and lands lane + pubkeys
   **together** on main.
3. Sitting `3` on each Mac from that main commit: re-pin, preflight, the
   S2-6 proof, the real-app proof.
4. Release from either Mac: one approval at Step 0.46. The signer then
   checks every file itself before signing it (slice S3): Linux daemons
   against GitHub (the tag, base + bump only, base on main, the build
   attestation from daemon-binaries.yml at the tag, the bytes);
   K2.app.tar.gz (Developer ID team 36B8R93HXV, stapled and notarized, the
   version, the signed daemon helper and k2-ptyd); the macOS daemon (arm64,
   one build marker for the version, the embedded k2-ptyd signed by the
   team); the Windows setup only when the approval named it (an NSIS installer, whose stub is a 32-bit PE even for x64;
   no independent evidence).

If z3mbp's sitting 2 slips past the landing, U2b needs a second pubkeys
commit and only releases after the one that ships it can be signed on z3mbp.

**Re-enrolling the approver** (e.g. after this change, or a new Mac login
password policy): `bash scripts/k2sign/sitting.sh reenrol --key U2 --commit <sha> --run`
rebuilds K2 Release Approve, runs `--enrol --replace` (the old key files are
renamed `*.old-<time>`), installs the new public key with
`sudo /usr/local/libexec/k2sign/k2sign-admin set-approver ~/.k2-release-approve/approver.pem`,
then proves Touch ID, the password, and (hands off) that an SSH-started request with nobody answering the sheet makes no approval.

**Rollback.**
- Soft: root sets `approval = off` in `/usr/local/etc/k2sign/policy` and
  runs `sudo /usr/local/libexec/k2sign/k2sign-admin repin`: no approval
  session; every other check still runs. Re-running `install.sh` restores
  the reviewed policy.
- Break-glass approval when Rosson can't reach the Mac's screen:
  `sudo /usr/local/libexec/k2sign/k2sign-admin approve <version> <base> <asset>…`
  (admin password; logged `APPROVED-BY-ADMIN`).
- Full: `K2_SIGNER=keychain` after the keys are exported back to
  `~/.k2-updater` (that export verb, `export-to-keychain`, is slice S4, not
  built yet; until then the keys can only be copied out by root by hand).
  With two primaries in the tree, set `K2_UPDATER_U2_PUB` to that Mac's key.

**S0 spike results** (the PRD's acceptance needs a written result for each):

| | What | Result |
|---|---|---|
| (a) | CryptoKit SE key, access control `[.privateKeyUsage, .biometryAny, .or, .devicePasscode]` (Touch ID or the login password; was `.biometryCurrentSet` until 2026-10-28), no keychain entitlement, ad hoc **and** Developer ID | **open:** needs the Mac + a person (`sitting.sh 0`); the password path is proven by `sitting.sh reenrol` (proof 2) |
| (b) | `open -W -n -a` shows the approval sheet from iTerm and from SSH; with nobody answering, an SSH-started request makes no approval | **z3mbpZ 2026-10-28:** an SSH-started helper's sheet appears on the console and approves only when answered there (reenrol runs 1–3, each with a touch); the hands-off rerun of proof 3 is open |
| (c) | LibreSSL 3.3.6 `openssl dgst -sha256 -verify` accepts CryptoKit's DER ECDSA | **pass** (2026-10-28, z3mbp, LibreSSL 3.3.6): a CryptoKit P-256 signature (software key: the SE key's DER and PEM formats are the same) verifies, a one-byte change is refused; the test suite then uses openssl-made P-256 signatures in the same format |
| (d) | gh attestation verify with only a read-only token in a scratch HOME | **pass on z3mbpZ (sitting 1, 2026-10-28):** `k2sign gh-selfcheck` ok as `_k2sign` with its own token. gh needs a token even for a public repo's attestations, and GitHub serves cli/cli's bundle by URL, so the signer lets the pinned gh fetch it. The token needs Contents + Attestations read-only on K2 |
| (e) | a 300 MB stdin stream through `sudo -n -u` | **open:** needs sudo in a VM (the Tart `k2-newuser-base` VM), never on a signing Mac |

A failure in (a) or (b) switches the approval to the passphrase fallback
(prd §11 Q1) before release use.

## What changes

| Key | What it is | Where it lives | Who trusts it |
|---|---|---|---|
| **U1** `E9111446F65EBB05` | today's key, now retired | `~/.k2-updater/k2-updater-u1.key` on z3mbpZ, password in the keychain (`k2-updater-signing-u1`) | installs **older than 0.46.6** only |
| **U2** (new) | the new primary | `~/.k2-updater/k2-updater-u2.key` on z3mbpZ, password in the keychain (`k2-updater-signing-u2`) | **0.46.6 and newer** |
| **U3** (new) | the offline backup | your password manager only, never on a box | **0.46.6 and newer** (if U2 is ever lost) |

v0.46.4 and v0.46.5 shipped signed with U1 only (this work slipped twice), so
0.46.6 is the first release the new keys sign (`FIRST_U2_SIGNED_VERSION` in
`crates/k2-release-keys`; the shell copies are pinned by its copies test).

From 0.46.6 every release is signed with **both** U1 and U2:
- old installs read the old fields (`signature` in `latest.json`, the daemon
  `.sig`) and keep updating;
- new installs read only the new fields (`k2_signatures`, the daemon
  `.sig2`) and trust only U2/U3.

So a stolen U1 can't make 0.46.6+ installs accept anything. The bridge (U1
signatures) ends in SS6, once K2 Web confirms the whole fleet is on 0.46.6 or
newer and at least 6 weeks after 0.46.6 shipped. That end is not part of this
runbook.

## Why the keychain and not the K2 vault (yet)

The goal was the K2 vault. Today it can't hand a person's release run a
secret just for one signing step:
- file and `env-file` delivery are for agent sessions only;
- a person reads a value with `k2 vault reveal`, which needs a Touch ID or
  password confirm every time, so an unattended release would stop and wait;
- `env-file` refuses values with line breaks, and macOS has no memory-backed
  folder for secret files.

What's missing is a small vault verb: for example `k2 vault use --secret NAME
--file --ttl 60 -- <cmd>` for the owner's own terminal, for secrets marked
"owner terminal, no per-use confirm", that writes a 0600 file in a private
folder, removes it afterwards, and logs every use. Until that exists, U2
follows the root-helper key (key file + keychain password). The swap is two
functions in `scripts/lib/sign-updater.sh` (`k2u_password`, `k2u_key_text`),
marked `TODO(vault)`.

## Before you start

- `brew install minisign` (0.11 or newer: `minisign -v`).
- A password manager entry ready for two new passwords.
- The checkout on z3mbpZ is on current main (it has
  `scripts/updater-key-ceremony.sh`).

## Steps

### 1. Move today's key U1 into place (for the bridge signatures)

```sh
mkdir -p ~/.k2-updater && chmod 700 ~/.k2-updater
cp ~/.tauri/k2-updater.key ~/.k2-updater/k2-updater-u1.key
chmod 600 ~/.k2-updater/k2-updater-u1.key
security add-generic-password -a "$USER" -s k2-updater-signing-u1 -w
```

The last command asks for U1's password (today it is
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` in the release checkout's `.env`).
Type it at the prompt; never put it on the command line. Then **delete that
line from `.env`**: `release.sh` refuses to run while it is there.

### 2. Make U2 and U3

Run it from a checkout of the `k2/sign-on-z3mbpz` branch (the keys land
together with the code; main doesn't have the script yet). It needs only
bash, python3, minisign and `security`:

```sh
git fetch origin && git checkout k2/sign-on-z3mbpz   # or the commit K2 names
bash scripts/updater-key-ceremony.sh generate
```

Stopping part-way (Ctrl-C, a typo, answering anything but `saved`) removes
everything that run made: the U3 folder (overwritten), U2's files and its
keychain item. Then start again.

It walks you through:
1. **U2**: minisign asks for a new password twice; the keychain asks for the
   same password once more. A self-test proves the keychain password opens
   the key.
2. **U3**: minisign asks for a different new password twice. The U3 secret
   key is written (still encrypted with U3's password) to
   `$TMPDIR/K2-U3-backup.XXXXXX/k2-updater-u3.key` in a private folder that
   Time Machine never backs up (also excluded with `tmutil addexclusion`),
   and a Finder window shows it. Make **two** password-manager entries:
   - "K2 update key U3 - key file": **attach** that file;
   - "K2 update key U3 - password": U3's password, and nothing else.

   Never the clipboard, never both in one entry. Type `saved`; the file is
   overwritten and deleted from z3mbpZ (best effort on an SSD; it is
   encrypted either way). U3's public key stays at
   `~/.k2-updater/k2-updater-u3.pub`.
3. It prints two **public** key lines (U2 and U3) with their key ids.

### 3. Send K2 the two public key lines

They're public; any channel is fine. K2 then runs, in a worktree:

```sh
bash scripts/updater-key-ceremony.sh pubkeys <U2 line> <U3 line>
```

This writes them into every verifier copy (`crates/k2-release-keys/src/lib.rs`,
`cli/k2`, `scripts/install-daemon.sh`, `src-tauri/tauri.conf.json`); the
`k2-release-keys` copies test checks they agree (release.sh Step 0.45 runs
it). K2 lands the keys together with the SS4/SS5 code, before the 0.46.6 tag:
this branch never lands on main without them, because a main build installed
before the keys exist could never self-update.

### 4. Prove it on z3mbpZ

After the public keys have landed (on this branch, or on main once it merges):

```sh
bash scripts/sign-daemon.sh --preflight 0.46.6
```

It checks both key files (0600 in a 0700 folder), both keychain items, a
self-test signature from each key against the keys in the source tree, and
that GitHub no longer has the `TAURI_SIGNING_*` secrets (SS1). Every
signature, including these self-tests, is logged in
`~/k2-release-audit/<version>/signatures.tsv`.

### 4b. The U3 drill (once now, then yearly)

Proves the backup can actually take over. On any Mac with minisign and a
checkout that has the public keys landed:

1. Save the "key file" attachment from the password manager to a temp path,
   for example `~/Desktop/u3.key`.
2. Run `bash scripts/updater-key-ceremony.sh drill ~/Desktop/u3.key` and type
   U3's password from the other entry when minisign asks.
3. It signs a throwaway file named `k2-u3-drill` (no installer accepts that
   name) and checks the signature with the **backup key in UPDATER_KEYS**.
   "U3 drill ok" means U3 restores and installs would accept it.
4. The drill deletes `~/Desktop/u3.key` itself when it ends (pass or
   fail); the password manager keeps the real copy.

### 4c. Re-running the Linux build by hand

The workflow uploads the daemons without `--clobber`, and `sign-daemon.sh`
checks the attestation's identity, which names the ref the workflow ran on.
So, for a re-run:

1. Delete the release's `k2-daemon-linux-<arch>` and
   `k2-daemon-linux-<arch>.sha256` (both arches you rebuild).
2. `gh workflow run daemon-binaries.yml --ref v<x> -f tag=v<x>` (the `--ref`
   must be the tag; running it from main gives an attestation for main,
   which signing refuses).
3. `bash scripts/sign-daemon.sh <x>` from the checkout that holds the tag.

### 5. Delete the GitHub secrets (SS1)

Since SS1 nothing on GitHub signs. Once SS1 is on main:

```sh
gh secret delete TAURI_SIGNING_PRIVATE_KEY -R Alakazam-211/K2
gh secret delete TAURI_SIGNING_PRIVATE_KEY_PASSWORD -R Alakazam-211/K2
```

(or Settings → Secrets and variables → Actions). The preflight fails while
they exist, unless `K2_ALLOW_GH_SIGNING_SECRETS=1`.

### 6. After the first green 0.46.6 release

```sh
rm ~/.tauri/k2-updater.key ~/.tauri/k2so-updater.key
```

## If something goes wrong

- **Typo in a password:** the self-test fails. Delete the keychain item
  (`security delete-generic-password -s k2-updater-signing-u2`) and
  `~/.k2-updater/k2-updater-u2.*`, then run `generate` again.
- **U2 lost (disk dies):** installs already trust U3, so nothing is
  stranded. Make a new backup U4, run `pubkeys <U3 line> <U4 line>` (U3
  becomes the primary), put U3's secret key from the password manager at
  `~/.k2-updater/k2-updater-u2.key` (the signing slot) and its password in
  the `k2-updater-signing-u2` keychain item, and release as usual.
- **U2 stolen:** an emergency release signed with U3 that drops U2 from
  `UPDATER_KEYS`, plus a new backup.

## Next: two independent builds (after 0.46.6, owner: K2)

0.46.6 signs a Linux daemon only when GitHub's build-provenance attestation
has the exact certificate identity of `daemon-binaries.yml` at
`refs/tags/v<x>` and its source commit is the commit of the LOCAL tag (the
checkout the Mac build came from), with GitHub's tag pointing at that same
commit (`sign-daemon.sh`). That stops a swapped release asset and a moved
tag. It does **not** stop a poisoned build:
a compromised self-hosted runner can build a bad daemon, attest it and pass
every byte check.

The real fix is to build each Linux daemon twice, on GitHub and on K2
Compute (different machines, different owners), and sign only when the two
hashes match. That needs reproducible builds: a pinned rustc, `--remap-path-prefix`,
no timestamps or absolute paths in the binary (plan-ci-release-on-k2-compute
G12; the shadow report already compares the two). Owner: K2 (this
workspace), after 0.46.6. Until then the residual risk is written down in the
SS1 threat notes.

## Threat notes (what 0.46.6 does and doesn't stop)

- **Key theft from GitHub (SS1):** stopped. No signing key on GitHub; the
  runner builds unsigned and z3mbpZ signs.
- **A swapped Linux release asset:** stopped. The real controls are the
  attestation (exact identity: `daemon-binaries.yml@refs/tags/v<x>`) and the
  local tag commit: the attestation's source commit and GitHub's tag must
  both equal the commit of `refs/tags/v<x>` in the checkout release.sh
  tagged. After signing, z3mbpZ re-downloads the published binaries and
  checks they are the bytes it signed. The uploader / upload-time checks
  are belts only: any workflow token uploads as `github-actions[bot]`, and
  GitHub never lists two assets with one name, so they prove little.
- **A poisoned build on the self-hosted runner:** NOT stopped. Next step:
  two independent builds (above), after 0.46.6.
- **Same-user read on z3mbpZ:** stopped by the signing user (above): no
  process running as Rosson's user can read U1/U2 or their passwords (only
  root can), and it can't use the signer as an oracle outside an approved
  session (one approval per release, Touch ID or the Mac login password, bound to version, base commit and
  assets; one sha256 per asset per version). Still open until slice S3: the
  signer doesn't yet check each file's origin itself, which is why it
  refuses every real file for now. Under the `K2_SIGNER=keychain` rollback,
  the old exposure is back (any same-user process can read
  `~/.k2-updater/*.key` and ask the login keychain for the passwords).
  Accepted and written down (prd §8): root (an admin password typed where a
  same-user process sees it), a poisoned Mac build (Developer ID and ASC
  keys stay in Rosson's user), a misleading approval prompt (limited by one
  session per version, monotonic versions and the per-file checks).
- **A stolen U1:** can't make 0.46.6+ installs accept anything (they trust
  only U2/U3). It still works on installs that never updated past 0.46.6,
  until the bridge ends (SS6).
- **Rollback:** a 0.46.6+ daemon refuses an older or the same version unless
  the owner asks for it (`allowDowngrade` / `reinstall`).
- **Linux packages (deb/rpm/AppImage):** still HTTPS + sha256 only, until SS7.
