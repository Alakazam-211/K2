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

```sh
bash scripts/updater-key-ceremony.sh generate
```

It walks you through:
1. **U2**: minisign asks for a new password twice; the keychain asks for the
   same password once more. A self-test proves the keychain password opens
   the key.
2. **U3**: minisign asks for a different new password twice. The U3 secret
   key file goes onto the clipboard: paste it into a password manager entry
   named "K2 update key U3 (offline backup)", together with U3's password.
   Press Enter; the clipboard is cleared and the U3 secret key is deleted
   from z3mbpZ. Its public key stays at `~/.k2-updater/k2-updater-u3.pub`.
3. It prints two **public** key lines (U2 and U3) with their key ids.

### 3. Send K2 the two public key lines

They're public; any channel is fine. K2 then runs, in a worktree:

```sh
bash scripts/updater-key-ceremony.sh pubkeys <U2 line> <U3 line>
```

This writes them into every verifier copy (`crates/k2-release-keys/src/lib.rs`,
`cli/k2`, `scripts/install-daemon.sh`, `src-tauri/tauri.conf.json`); the
`k2-release-keys` copies test checks they agree. K2 lands it on main before
the 0.46.4 tag.

### 4. Prove it on z3mbpZ

After pulling main:

```sh
bash scripts/sign-daemon.sh --preflight 0.46.4
```

It checks both key files (0600 in a 0700 folder), both keychain items, a
self-test signature from each key against the keys in the source tree, and
that GitHub no longer has the `TAURI_SIGNING_*` secrets (SS1). Every
signature, including these self-tests, is logged in
`~/k2-release-audit/<version>/signatures.tsv`.

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
