# K2 on Arch / Omarchy

Whole bundle: **GUI + `k2-daemon` + `cli/k2` + systemd user unit**. Not daemon-only, not a `.deb`.

This is the Omarchy/Arch artifact (`PKGBUILD` → `.pkg.tar.zst`). GitHub's x86_64 deb/rpm/AppImage (`app-linux.yml`) is a different SKU — do not `debtap` it.

## Install from this tree

Needs a native **Linux x86_64** Arch/Omarchy box (WebKitGTK at link; no macOS cross-compile of the GUI).

```sh
# base-devel + runtime/build deps
sudo pacman -S --needed base-devel rust clang cmake pkgconf git bun \
    webkit2gtk-4.1 gtk3 libayatana-appindicator librsvg libsecret openssl

# Source-build bakes Gmail OAuth at compile time (option_env).
# Never commit these. Repo-root `.env` is gitignored and sourced if present.
export K2_GMAIL_CLIENT_ID=...
export K2_GMAIL_CLIENT_SECRET=...

cd packaging/arch
makepkg -si
```

`makepkg` copies the parent git tree (two levels up). Run it from `packaging/arch/`, not via a Debian helper.

### After install

```sh
# Pacman IS the install. Do NOT run `k2 daemon install`
# (that fetches a second GitHub binary and writes leftover k2so-daemon names).
systemctl --user daemon-reload
systemctl --user enable --now k2-daemon
# Headless: loginctl enable-linger "$USER"

k2 whoami                  # PATH /usr/bin/k2 = bash CLI (cli/k2)
# GUI: k2-gui, or the "K2" desktop entry (Exec=k2-gui)
# Smoke: curl the daemon /boot-status — do not use `k2 daemon status` (Darwin-only).
```

Upgrade with pacman/AUR. Do not teach GitHub Shape B swap, a Tauri Linux updater, or `k2 update` overwrite for this install.

### Uninstall

```sh
sudo pacman -R k2
# ~/.k2 is left in place on purpose.
```

## Paths (O14 / O15 / O26)

| Path | What |
|---|---|
| `/usr/bin/k2` | bash CLI from `cli/k2` |
| `/usr/bin/k2-gui` | symlink → `/usr/lib/k2/k2-gui` (Tauri/WebKitGTK) |
| `/usr/lib/k2/k2-gui` | GUI binary (crate bin is still `k2` so Darwin `Contents/MacOS/k2` stays put) |
| `/usr/lib/k2/frpc` | Connect sidecar next to the GUI (not `/usr/bin/frpc`) |
| `/usr/bin/k2-daemon` | daemon |
| `/usr/lib/systemd/user/k2-daemon.service` | **user** unit, `ExecStart=/usr/bin/k2-daemon`. Not auto-enabled. Not `k2so-daemon`. Not a system unit. |

## Depends (until a real Omarchy `ldd`)

`webkit2gtk-4.1`, `gtk3` (**not gtk4**), `libayatana-appindicator`, `librsvg`, `libsecret`, `openssl`.

## OAuth (O25)

Source PKGBUILD **must** bake Gmail via `scripts/require-mail-oauth-build-env.sh` or the daemon ships `REPLACE_ME` and Google returns `invalid_client`.

A later `k2-bin` package may consume the already-baked GitHub `k2-daemon-linux-x86_64` instead of compiling the daemon. This PKGBUILD builds from this tree.

## Out of this package

- Air-gap `--features airgap` (separate SKU, never this package).
- Folding Omarchy into `scripts/release.sh` / GitHub Release `.pkg.tar.zst` until a real Omarchy box has `makepkg -si` (O9).
- Linux GUI auto-update / `latest.json` linux keys.
- `k2 daemon status|start|stop` lifecycle CLI (Darwin/`launchctl`).
- `pkgs.omarchy.org` (later ask). aarch64 GUI (same PKGBUILD `arch=()` later).
- AUR name: `k2` here; rename to `k2-bin` at submit if `k2` is taken.

## Wayland / Hyprland

Omarchy default is Wayland. No `WEBKIT_*` / GSK env hacks until a real window smoke fails.

## License

`FSL-1.1-Apache-2.0` — see `LICENSE.md`.
