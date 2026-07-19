# macOS production release runbook

This runbook keeps artifact verification separate from installation and
activation. Build and verification commands must not start K2 or rewrite the
shared `dev.k2.daemon` launchd registration.

## 1. Build a signed, notarized native artifact

From a clean checkout on the target architecture:

```sh
./scripts/build-local.sh <version>
```

The checked-in version files must already match `<version>`. To check the
source/version gate without loading credentials or starting a build:

```sh
K2_PREFLIGHT_ONLY=1 ./scripts/build-local.sh <version>
```

`K2_ALLOW_DIRTY=1` permits a development-only signed candidate. Its provenance
is marked dirty, and the production verifier rejects it by default.

The build writes an architecture-stamped DMG and provenance JSON. Provenance
records the source commit, source-tree state, native architecture, and SHA-256
hashes for the DMG, `k2`, `k2-daemon`, and `frpc`. The build fails unless all
three executables contain the native architecture, versions agree, signing
verification passes, and Apple accepts and staples both the app and DMG.

## 2. Verify without starting either runtime

Run from the repository root:

```sh
set -euo pipefail
VERSION="${K2_RELEASE_VERSION:?set K2_RELEASE_VERSION}"
ARCH="$(./scripts/macos-native-arch.sh)"
COMMIT="$(git rev-parse HEAD)"
APP="target/release/bundle/macos/K2.app"
PROVENANCE="target/release/bundle/dmg/K2_${VERSION}_${ARCH}.provenance.json"

./scripts/verify-macos-bundle.sh "$APP" "$ARCH" "$PROVENANCE" "$COMMIT"
codesign --verify --deep --strict --verbose=2 "$APP"
xcrun stapler validate "$APP"
spctl --assess --type execute --verbose=2 "$APP"
```

The verifier uses only the app's early `--version` path and the daemon's
guarded artifact-version probe. Both exit before Tauri, Tokio, filesystem,
database, listener, migration, or launchd work. A legacy daemon without the
SHA-bound probe marker is rejected without execution.

## 3. Prepare immutable paired staging

Staging copies a verified app and provenance into a never-reused release ID,
removes write permission, and atomically renames the temporary directory. It
does not change launchd or start K2.

```sh
set -euo pipefail
VERSION="${K2_RELEASE_VERSION:?set K2_RELEASE_VERSION}"
ARCH="${K2_RELEASE_ARCH:?set K2_RELEASE_ARCH}"
COMMIT="${K2_RELEASE_COMMIT:?set K2_RELEASE_COMMIT}"
RELEASES_DIR="${K2_RELEASES_DIR:-$HOME/Library/Application Support/K2/releases}"

[ "$(git rev-parse HEAD)" = "$COMMIT" ]
APP_SOURCE="$PWD/target/release/bundle/macos/K2.app"
PROVENANCE="$PWD/target/release/bundle/dmg/K2_${VERSION}_${ARCH}.provenance.json"
SHORT_COMMIT="$(git rev-parse --short=12 HEAD)"
RELEASE_ROOT="$RELEASES_DIR/${VERSION}-${ARCH}-${SHORT_COMMIT}"
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

Installation, launchd cutover, and rollback are deployment-specific operations.
Keep their approval, interruption, health, and rollback procedures in the
deployment environment rather than this portable build runbook.

## 4. Public multi-architecture release design

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

Keep this follow-up separate from the Intel build-safety change so signing
ownership and publication permissions can be reviewed explicitly.
