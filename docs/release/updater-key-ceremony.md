# Update-key ceremony (U1 → U2, with an offline backup U3)

Runbook for the planned rotation of K2's update-signing key
(prd-security-sweep-0464-v1, SS4/SS5, option C "dual-sign bridge").
Rosson runs it **once**, on **z3mbpZ**, in a **GUI terminal** (iTerm or
Terminal, not SSH). It takes about 15 minutes, and it must be done before
0.46.4 can be released: `release.sh` refuses to start while the new key is
missing.

## What changes

| Key | What it is | Where it lives | Who trusts it |
|---|---|---|---|
| **U1** `E9111446F65EBB05` | today's key, now retired | `~/.k2-updater/k2-updater-u1.key` on z3mbpZ, password in the keychain (`k2-updater-signing-u1`) | installs **older than 0.46.4** only |
| **U2** (new) | the new primary | `~/.k2-updater/k2-updater-u2.key` on z3mbpZ, password in the keychain (`k2-updater-signing-u2`) | **0.46.4 and newer** |
| **U3** (new) | the offline backup | your password manager only, never on a box | **0.46.4 and newer** (if U2 is ever lost) |

From 0.46.4 every release is signed with **both** U1 and U2:
- old installs read the old fields (`signature` in `latest.json`, the daemon
  `.sig`) and keep updating;
- new installs read only the new fields (`k2_signatures`, the daemon
  `.sig2`) and trust only U2/U3.

So a stolen U1 can't make 0.46.4+ installs accept anything. The bridge (U1
signatures) ends in SS6, once K2 Web confirms the whole fleet is on 0.46.4 or
newer and at least 6 weeks after 0.46.4 shipped. That end is not part of this
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
   `~/K2-U3-backup.XXXXXX/k2-updater-u3.key` in a private folder, and a
   Finder window shows it. Make **two** password-manager entries:
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
it). K2 lands the keys together with the SS4/SS5 code, before the 0.46.4 tag:
this branch never lands on main without them, because a main build installed
before the keys exist could never self-update.

### 4. Prove it on z3mbpZ

After the public keys have landed (on this branch, or on main once it merges):

```sh
bash scripts/sign-daemon.sh --preflight 0.46.4
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
4. Delete `~/Desktop/u3.key` (and empty the Trash).

### 5. Delete the GitHub secrets (SS1)

Since SS1 nothing on GitHub signs. Once SS1 is on main:

```sh
gh secret delete TAURI_SIGNING_PRIVATE_KEY -R Alakazam-211/K2
gh secret delete TAURI_SIGNING_PRIVATE_KEY_PASSWORD -R Alakazam-211/K2
```

(or Settings → Secrets and variables → Actions). The preflight fails while
they exist, unless `K2_ALLOW_GH_SIGNING_SECRETS=1`.

### 6. After the first green 0.46.4 release

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

## Next: two independent builds (0.46.5, owner: K2)

0.46.4 signs a Linux daemon only when GitHub's build-provenance attestation
says it came out of `daemon-binaries.yml` at the tag's commit, and the
release asset was uploaded once by `github-actions[bot]` (`sign-daemon.sh`).
That stops a swapped release asset. It does **not** stop a poisoned build:
a compromised self-hosted runner can build a bad daemon, attest it and pass
every byte check.

The real fix is to build each Linux daemon twice, on GitHub and on K2
Compute (different machines, different owners), and sign only when the two
hashes match. That needs reproducible builds: a pinned rustc, `--remap-path-prefix`,
no timestamps or absolute paths in the binary (plan-ci-release-on-k2-compute
G12; the shadow report already compares the two). Owner: K2 (this
workspace), for 0.46.5. Until then the residual risk is written down in the
SS1 threat notes.

## Threat notes (what 0.46.4 does and doesn't stop)

- **Key theft from GitHub (SS1):** stopped. No signing key on GitHub; the
  runner builds unsigned and z3mbpZ signs.
- **A swapped Linux release asset:** stopped. z3mbpZ signs only an attested
  build from `daemon-binaries.yml` at the tag's commit, uploaded once by
  `github-actions[bot]`, and re-checks the published bytes after signing.
- **A poisoned build on the self-hosted runner:** NOT stopped. Next step:
  two independent builds (above), 0.46.5.
- **Same-user read on z3mbpZ:** NOT stopped, accepted for 0.46.4 pending
  Rosson's OK. Any process running as Rosson's macOS user (a build script, a
  package, an agent) can read `~/.k2-updater/*.key` and ask the unlocked
  login keychain for the passwords, exactly like the root-helper key today.
  The release run itself never puts the keys in an environment or on argv.
  Real fix: SS2b, a separate `k2sign` macOS user that holds the keys and
  only signs (0.46.5).
- **A stolen U1:** can't make 0.46.4+ installs accept anything (they trust
  only U2/U3). It still works on installs that never updated past 0.46.3,
  until the bridge ends (SS6).
- **Rollback:** a 0.46.4+ daemon refuses an older or the same version unless
  the owner asks for it (`allowDowngrade` / `reinstall`).
- **Linux packages (deb/rpm/AppImage):** still HTTPS + sha256 only, until SS7.
