# macOS production release runbook

This runbook keeps macOS build verification separate from production activation.
Build and test commands must never rewrite the shared `dev.k2.daemon` launchd
registration. A production cutover always promotes a version-matched `K2.app`
and `k2-daemon` from the same immutable bundle.

## 1. Build a signed, notarized native artifact

Run on the target architecture:

```sh
./scripts/build-local.sh <version>
```

The checked-in version files must already match `<version>`. The production
path requires a clean tree and never rewrites source during the build. For an
explicitly development-only signed candidate, `K2_ALLOW_DIRTY=1` is available;
its provenance is marked dirty and the production verifier rejects it.

To check the source/version gate without loading credentials or starting a
build:

```sh
K2_PREFLIGHT_ONLY=1 ./scripts/build-local.sh <version>
```

The build writes an architecture-stamped DMG and a provenance JSON file. The
provenance records the source commit, whether the source tree was dirty before
the build, the native architecture, and SHA-256 hashes for the DMG, `k2`,
`k2-daemon`, and `frpc`. The build fails unless all three executables contain
the native architecture, the bundle and executable versions agree, code-signing
verification passes, and Apple accepts and staples both the app and DMG.

An artifact with `source_dirty_before_build: true` is a development candidate,
not a public release. Preserve its exact diff with the review evidence.

## 2. Verify without starting either runtime

```sh
set -euo pipefail
REPO_ROOT="${K2_REPO_ROOT:-$HOME/Projects/lab/k2-intel-build}"
VERSION="${K2_RELEASE_VERSION:?set K2_RELEASE_VERSION to the approved version}"
cd "$REPO_ROOT"
ARCH="$(./scripts/macos-native-arch.sh)"
COMMIT="$(git rev-parse HEAD)"
APP_SOURCE="$PWD/target/release/bundle/macos/K2.app"
PROVENANCE="$PWD/target/release/bundle/dmg/K2_${VERSION}_${ARCH}.provenance.json"

./scripts/verify-macos-bundle.sh "$APP_SOURCE" "$ARCH" "$PROVENANCE" "$COMMIT"
codesign --verify --deep --strict --verbose=2 \
  "$APP_SOURCE"
xcrun stapler validate "$APP_SOURCE"
spctl --assess --type execute --verbose=2 "$APP_SOURCE"
```

The verifier runs only the app's early `--version` path and the daemon's guarded
artifact-version probe. Both exit before Tauri, Tokio, filesystem, database,
listener, migration, or launchd work. The verifier refuses to execute a legacy
daemon that does not contain the guarded probe.

## 3. Prepare immutable paired staging

These commands stage into a temporary sibling, verify the copied app against
the provenance hashes, remove write permission, and atomically rename it to a
never-reused release ID. They do not change launchd or start K2. Staging writes
files and recursively changes their permissions, so show this exact populated
block and obtain approval before running it.

```sh
set -euo pipefail
REPO_ROOT="${K2_REPO_ROOT:-$HOME/Projects/lab/k2-intel-build}"
VERSION="${K2_RELEASE_VERSION:?set K2_RELEASE_VERSION}"
ARCH="${K2_RELEASE_ARCH:?set K2_RELEASE_ARCH}"
COMMIT="${K2_RELEASE_COMMIT:?set K2_RELEASE_COMMIT}"
cd "$REPO_ROOT"
[ "$(git rev-parse HEAD)" = "$COMMIT" ]
APP_SOURCE="$REPO_ROOT/target/release/bundle/macos/K2.app"
PROVENANCE="$REPO_ROOT/target/release/bundle/dmg/K2_${VERSION}_${ARCH}.provenance.json"
SHORT_COMMIT="$(git rev-parse --short=12 HEAD)"
RELEASE_ID="${VERSION}-${ARCH}-${SHORT_COMMIT}"
RELEASE_ROOT="$HOME/Library/Application Support/K2/releases/$RELEASE_ID"
STAGE_ROOT="${RELEASE_ROOT}.tmp.$$"

test ! -e "$RELEASE_ROOT"
test ! -e "$STAGE_ROOT"
mkdir -p "$STAGE_ROOT"
ditto "$APP_SOURCE" "$STAGE_ROOT/K2.app"
cp "$PROVENANCE" "$STAGE_ROOT/provenance.json"
./scripts/verify-macos-bundle.sh \
  "$STAGE_ROOT/K2.app" "$ARCH" "$STAGE_ROOT/provenance.json" "$COMMIT"
codesign --verify --deep --strict --verbose=2 "$STAGE_ROOT/K2.app"
xcrun stapler validate "$STAGE_ROOT/K2.app"
chmod -R a-w "$STAGE_ROOT"
test ! -e "$RELEASE_ROOT"
mv "$STAGE_ROOT" "$RELEASE_ROOT"
printf 'K2_RELEASE_ROOT=%s\n' "$RELEASE_ROOT"
```

Record the absolute staged path, provenance JSON, `git diff`, test output, and
the three executable hashes in the approval request.

## 4. Snapshot rollback state

Do this immediately before requesting cutover approval. It copies state but does
not stop or restart anything.

```sh
set -euo pipefail
REPO_ROOT="${K2_REPO_ROOT:-$HOME/Projects/lab/k2-intel-build}"
ARCH="${K2_RELEASE_ARCH:?set K2_RELEASE_ARCH}"
MINECRAFT_PID="${K2_MINECRAFT_PID:?set K2_MINECRAFT_PID to the verified live PID}"
cd "$REPO_ROOT"
LABEL=dev.k2.daemon
DOMAIN="gui/$(id -u)"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
ROLLBACK_ROOT="$HOME/Library/Application Support/K2/rollback/$(date -u +%Y%m%d-%H%M%S)"

kill -0 "$MINECRAFT_PID"
test -f "$PLIST"
PRIOR_DAEMON="$(/usr/libexec/PlistBuddy -c 'Print :ProgramArguments:0' "$PLIST")"
PRIOR_APP="${PRIOR_DAEMON%/Contents/MacOS/k2-daemon}"
test -x "$PRIOR_DAEMON"
test -x "$PRIOR_APP/Contents/MacOS/k2"
LOADED_DAEMON="$(launchctl print "$DOMAIN/$LABEL" | sed -n 's/^[[:space:]]*program = //p' | head -1)"
[ "$LOADED_DAEMON" = "$PRIOR_DAEMON" ]
./scripts/verify-macos-bundle.sh "$PRIOR_APP" "$ARCH"
mkdir -p "$ROLLBACK_ROOT"
cp "$PLIST" "$ROLLBACK_ROOT/$LABEL.plist"
printf '%s\n' "$PRIOR_DAEMON" > "$ROLLBACK_ROOT/prior-daemon-path.txt"
printf '%s\n' "$PRIOR_APP" > "$ROLLBACK_ROOT/prior-app-path.txt"
printf '%s\n' "$MINECRAFT_PID" > "$ROLLBACK_ROOT/minecraft-pid.txt"
shasum -a 256 "$PLIST" > "$ROLLBACK_ROOT/plist.sha256"
```

The first transition from a legacy daemon without the guarded version probe
must stop at the verifier and use a separately reviewed, already-proven rollback
snapshot. Never test a legacy daemon by executing it with an unknown flag.

## 5. Promotion gate

The commands below interrupt active K2 UI/WebSocket clients and change the live
app/daemon pair. They do not operate on Minecraft, but the exact Minecraft PID
is checked before and after. Show the populated block, staged hashes, rollback
directory, current health, and interruption warning to the operator and obtain
explicit approval before running it.

```sh
set -euo pipefail
REPO_ROOT="${K2_REPO_ROOT:-$HOME/Projects/lab/k2-intel-build}"
VERSION="${K2_RELEASE_VERSION:?set K2_RELEASE_VERSION}"
ARCH="${K2_RELEASE_ARCH:?set K2_RELEASE_ARCH}"
COMMIT="${K2_RELEASE_COMMIT:?set K2_RELEASE_COMMIT}"
RELEASE_ROOT="${K2_RELEASE_ROOT:?set K2_RELEASE_ROOT to the approved immutable directory}"
ROLLBACK_ROOT="${K2_ROLLBACK_ROOT:?set K2_ROLLBACK_ROOT to the approved snapshot}"
MINECRAFT_PID="${K2_MINECRAFT_PID:?set K2_MINECRAFT_PID to the verified live PID}"
cd "$REPO_ROOT"
LABEL=dev.k2.daemon
DOMAIN="gui/$(id -u)"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
NEW_APP="$RELEASE_ROOT/K2.app"
NEW_DAEMON="$NEW_APP/Contents/MacOS/k2-daemon"
NEW_PLIST="$ROLLBACK_ROOT/$LABEL.new.plist"

case "$RELEASE_ROOT" in
  "$HOME/Library/Application Support/K2/releases/"*) ;;
  *) echo "unexpected release root: $RELEASE_ROOT" >&2; exit 1 ;;
esac
test -f "$ROLLBACK_ROOT/$LABEL.plist"
[ "$(cat "$ROLLBACK_ROOT/minecraft-pid.txt")" = "$MINECRAFT_PID" ]
test -x "$NEW_APP/Contents/MacOS/k2"
test -x "$NEW_DAEMON"
kill -0 "$MINECRAFT_PID"
./scripts/verify-macos-bundle.sh \
  "$NEW_APP" "$ARCH" "$RELEASE_ROOT/provenance.json" "$COMMIT"
codesign --verify --deep --strict --verbose=2 "$NEW_APP"
xcrun stapler validate "$NEW_APP"
cp "$PLIST" "$NEW_PLIST"
/usr/libexec/PlistBuddy -c "Set :ProgramArguments:0 $NEW_DAEMON" "$NEW_PLIST"
plutil -lint "$NEW_PLIST"

if pgrep -x k2 >/dev/null; then
  osascript -e 'tell application "K2" to quit'
fi
for _ in 1 2 3 4 5 6 7 8 9 10; do
  pgrep -x k2 >/dev/null || break
  sleep 1
done
if pgrep -x k2 >/dev/null; then
  echo "K2 app did not quit; aborting before daemon cutover" >&2
  exit 1
fi
launchctl bootout "$DOMAIN/$LABEL"
cp "$NEW_PLIST" "$PLIST.new"
mv -f "$PLIST.new" "$PLIST"
launchctl bootstrap "$DOMAIN" "$PLIST"
launchctl kickstart -k "$DOMAIN/$LABEL"
open "$NEW_APP"

APP_PID=""
for _ in 1 2 3 4 5 6 7 8 9 10; do
  if APP_PID="$(pgrep -nx k2)"; then break; fi
  sleep 1
done
[ -n "$APP_PID" ]
RUNNING_APP="$(ps -p "$APP_PID" -o comm= | sed 's/^[[:space:]]*//')"
[ "$RUNNING_APP" = "$NEW_APP/Contents/MacOS/k2" ]
NEW_PID="$(launchctl print "$DOMAIN/$LABEL" | awk '$1 == "pid" && $2 == "=" {print $3; exit}')"
RUNNING_DAEMON="$(ps -p "$NEW_PID" -o comm= | sed 's/^[[:space:]]*//')"
[ "$RUNNING_DAEMON" = "$NEW_DAEMON" ]
PORT="$(lsof -nP -a -p "$NEW_PID" -iTCP -sTCP:LISTEN | awk 'NR > 1 {sub(/^.*:/, "", $9); print $9; exit}')"
[ "$(curl -fsS "http://127.0.0.1:$PORT/health")" = '{"status":"ok"}' ]
[ "$(curl -fsS "http://127.0.0.1:$PORT/ping")" = "k2-daemon $VERSION (tokio)" ]
kill -0 "$MINECRAFT_PID"
if pgrep -f "$PWD/target/release/bundle/macos/K2.app/Contents/MacOS/k2" >/dev/null; then
  echo "worktree-owned K2 process detected" >&2
  exit 1
fi
```

Do not call the cutover successful until the K2 clients reconnect and the
operator verifies the persisted session/chat list in addition to the automated
checks above. Do not claim continuity merely because the processes are healthy.

## 6. Rollback

If any promotion check fails, restore the complete previous pair:

```sh
set -euo pipefail
ROLLBACK_ROOT="${K2_ROLLBACK_ROOT:?set K2_ROLLBACK_ROOT to the approved snapshot}"
LABEL=dev.k2.daemon
DOMAIN="gui/$(id -u)"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
PRIOR_APP="$(cat "$ROLLBACK_ROOT/prior-app-path.txt")"
PRIOR_DAEMON="$(cat "$ROLLBACK_ROOT/prior-daemon-path.txt")"
MINECRAFT_PID="$(cat "$ROLLBACK_ROOT/minecraft-pid.txt")"
test -x "$PRIOR_APP/Contents/MacOS/k2"
test -x "$PRIOR_DAEMON"
plutil -lint "$ROLLBACK_ROOT/$LABEL.plist"
kill -0 "$MINECRAFT_PID"
if pgrep -x k2 >/dev/null; then
  osascript -e 'tell application "K2" to quit'
fi
if launchctl print "$DOMAIN/$LABEL" >/dev/null 2>&1; then
  launchctl bootout "$DOMAIN/$LABEL"
fi
cp "$ROLLBACK_ROOT/$LABEL.plist" "$PLIST.new"
mv -f "$PLIST.new" "$PLIST"
launchctl bootstrap "$DOMAIN" "$PLIST"
launchctl kickstart -k "$DOMAIN/$LABEL"
open "$PRIOR_APP"

APP_PID=""
for _ in 1 2 3 4 5 6 7 8 9 10; do
  if APP_PID="$(pgrep -nx k2)"; then break; fi
  sleep 1
done
[ -n "$APP_PID" ]
RUNNING_APP="$(ps -p "$APP_PID" -o comm= | sed 's/^[[:space:]]*//')"
[ "$RUNNING_APP" = "$PRIOR_APP/Contents/MacOS/k2" ]
ROLLBACK_PID="$(launchctl print "$DOMAIN/$LABEL" | awk '$1 == "pid" && $2 == "=" {print $3; exit}')"
RUNNING_DAEMON="$(ps -p "$ROLLBACK_PID" -o comm= | sed 's/^[[:space:]]*//')"
[ "$RUNNING_DAEMON" = "$PRIOR_DAEMON" ]
PORT="$(lsof -nP -a -p "$ROLLBACK_PID" -iTCP -sTCP:LISTEN | awk 'NR > 1 {sub(/^.*:/, "", $9); print $9; exit}')"
[ "$(curl -fsS "http://127.0.0.1:$PORT/health")" = '{"status":"ok"}' ]
kill -0 "$MINECRAFT_PID"
```

Verify K2 client reconnect and persisted sessions after rollback as well.

## 7. Public multi-architecture release design

The current `release.sh` deliberately remains ARM-only and fails closed on an
Intel host. Public Intel support needs a coordinated follow-up:

1. Build, sign, and notarize separate ARM and Intel artifacts from the same
   version and source commit. Build jobs must not publish.
2. Give every app archive, DMG, daemon, signature, and provenance file an
   architecture-stamped name.
3. A single merge job validates both provenances and generates one `latest.json`
   containing `darwin-aarch64` and `darwin-x86_64`.
4. The same merge produces one `daemon-latest.json` containing
   `macos-aarch64`, `macos-x86_64`, and the existing Linux entries.
5. Only the merge/publish job may create the tag or release. If either macOS
   architecture is absent or mismatched, publication fails without changing the
   existing release.

Keep this follow-up separate from the Intel build-safety PR so the maintainer can
review signing ownership and publication permissions explicitly.
