# K2 in Omarchy's Install > Package list

`k2-bin/` is the recipe for [`omacom/omarchy-pkgs`](https://github.com/omacom/omarchy-pkgs): copy it to `pkgbuilds/k2-bin/` in a fork and open a PR with that one package. It is kept here so our checks catch drift. Nothing in this repo publishes it.

- **No build.** The recipe unpacks our GitHub `k2-<version>-x86_64.pkg.tar.zst`, pinned by sha256. Their host builds the package and signs it with Omarchy's key, so no key or secret of ours is involved.
- **Updates.** Their bot watches our GitHub releases and bumps `pkgver` and the checksums. `min_release_age: 24h` covers the time our package takes to reach a new release (about 45 minutes), and gives us a day to pull a bad one.
- **Our promise.** A published package never changes bytes (`scripts/arch-upload-pkg.sh`). A release whose Arch build failed has no package until the build is re-run; their sync fails on that version until then, and users stay on the previous one.

## Checks

```sh
bash scripts/check-omarchy-recipe.sh                # text checks, on every push
bash tests/packaging/check-omarchy-recipe.sh        # the check fails on broken fixtures
bash tests/packaging/omarchy-recipe-build.sh        # on an Arch machine: really build it
```

## After a change to `packaging/arch/k2.install`

Copy it to `k2-bin/k2-bin.install` and update that file's sha256 in `k2-bin/PKGBUILD`. The check fails until you do. A recipe already in `omarchy-pkgs` needs the same change there.
