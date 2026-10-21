#!/usr/bin/env python3
"""Snapshot, verify, merge, and check an optional Intel Tauri update."""

import base64
import json
import os
import re
import secrets
import shutil
import stat
import subprocess
import sys
import tempfile
from urllib.parse import urlparse


OWNER_FILE = ".k2-intel-updater-owner"
TOKEN_RE = re.compile(r"[0-9a-f]{64}")


def die(message):
    raise SystemExit(f"ERROR: {message}")


def no_duplicate_keys(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            die(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def load_json(path):
    try:
        with (sys.stdin if path == "-" else open(path, encoding="utf-8")) as source:
            return json.load(source, object_pairs_hook=no_duplicate_keys)
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        die(f"invalid JSON in {path}: {error}")


def regular(path, label, readonly=False):
    try:
        info = os.lstat(path)
    except OSError as error:
        die(f"Intel {label} is unavailable: {error}")
    if not stat.S_ISREG(info.st_mode):
        die(f"Intel {label} must be a regular non-symlink file: {path}")
    if info.st_size == 0:
        die(f"Intel {label} is empty: {path}")
    if readonly and info.st_mode & 0o222:
        die(f"staged Intel {label} must be read-only: {path}")
    return info


def validate_names(version, archive, signature_path, url):
    name = f"K2_{version}_x86_64.app.tar.gz"
    if os.path.basename(archive) != name:
        die(f"Intel archive must be named {name}")
    if signature_path != f"{archive}.sig":
        die("Intel signature must be the archive path plus .sig")
    parsed = urlparse(url)
    suffix = f"/releases/download/v{version}/{name}"
    if parsed.scheme != "https" or not parsed.netloc or not parsed.path.endswith(suffix) \
            or parsed.params or parsed.query or parsed.fragment:
        die(f"malformed Intel updater URL: {url}")
    return name


def copy_snapshot(source, destination, label):
    before = regular(source, label)
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    try:
        source_fd = os.open(source, flags)
        opened = os.fstat(source_fd)
        if (before.st_dev, before.st_ino) != (opened.st_dev, opened.st_ino) \
                or not stat.S_ISREG(opened.st_mode):
            die(f"Intel {label} changed while opening: {source}")
        destination_fd = os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o400)
        with os.fdopen(source_fd, "rb") as src, os.fdopen(destination_fd, "wb") as dst:
            shutil.copyfileobj(src, dst)
            dst.flush()
            os.fsync(dst.fileno())
    except OSError as error:
        die(f"could not snapshot Intel {label}: {error}")
    regular(destination, label, readonly=True)


def signature_text(path):
    try:
        with open(path, encoding="ascii") as source:
            wrapped = source.read().strip()
        raw = base64.b64decode(wrapped, validate=True).decode("utf-8")
    except (OSError, UnicodeError, ValueError) as error:
        die(f"malformed Intel signature: {error}")
    if not raw.startswith("untrusted comment"):
        die("Intel signature is not a Tauri-wrapped minisign signature")
    return wrapped, raw


def updater_pubkey(config_path):
    data = load_json(config_path)
    try:
        wrapped = data["plugins"]["updater"]["pubkey"]
        raw = base64.b64decode(wrapped, validate=True).decode("utf-8")
    except (KeyError, TypeError, UnicodeError, ValueError) as error:
        die(f"invalid updater pubkey in {config_path}: {error}")
    if not raw.startswith("untrusted comment"):
        die(f"invalid updater pubkey in {config_path}")
    return raw


def verify_crypto(archive, signature_path, config_path):
    archive_info = regular(archive, "archive")
    signature_info = regular(signature_path, "signature")
    if (archive_info.st_dev, archive_info.st_ino) == (signature_info.st_dev, signature_info.st_ino):
        die("Intel archive and signature must be distinct files")
    _, raw_signature = signature_text(signature_path)
    raw_pubkey = updater_pubkey(config_path)
    verifier = shutil.which("minisign")
    if verifier is None:
        die("minisign verifier is required for Intel updater input")
    with tempfile.TemporaryDirectory(prefix="k2-intel-verify-") as verify_dir:
        signature = os.path.join(verify_dir, "archive.minisig")
        pubkey = os.path.join(verify_dir, "updater.pub")
        with open(signature, "x", encoding="utf-8") as output:
            output.write(raw_signature)
        with open(pubkey, "x", encoding="utf-8") as output:
            output.write(raw_pubkey)
        result = subprocess.run(
            [verifier, "-Vm", archive, "-p", pubkey, "-x", signature],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
        )
    if result.returncode:
        die(f"Intel archive minisign verification failed: {result.stderr.strip()}")


def stage(version, archive, signature_path, url, config_path):
    name = validate_names(version, archive, signature_path, url)
    archive_info = regular(archive, "archive")
    signature_info = regular(signature_path, "signature")
    if (archive_info.st_dev, archive_info.st_ino) == (signature_info.st_dev, signature_info.st_ino):
        die("Intel archive and signature must be distinct files")
    stage_dir = tempfile.mkdtemp(prefix="k2-intel-updater-")
    staged_archive = os.path.join(stage_dir, name)
    staged_signature = f"{staged_archive}.sig"
    marker = os.path.join(stage_dir, OWNER_FILE)
    token = secrets.token_hex(32)
    try:
        marker_fd = os.open(
            marker,
            os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0),
            0o400,
        )
        with os.fdopen(marker_fd, "w", encoding="ascii") as output:
            output.write(f"{token}\n")
            output.flush()
            os.fsync(output.fileno())
        copy_snapshot(archive, staged_archive, "archive")
        copy_snapshot(signature_path, staged_signature, "signature")
        verify_crypto(staged_archive, staged_signature, config_path)
        os.chmod(marker, 0o444)
        os.chmod(staged_archive, 0o444)
        os.chmod(staged_signature, 0o444)
        os.chmod(stage_dir, 0o555)
    except BaseException:
        try:
            discard_partial_stage(stage_dir, token, name)
        except BaseException:
            print("ERROR: secondary Intel staging cleanup failed", file=sys.stderr)
        raise
    print(f"{stage_dir}\t{token}")


def stage_location(stage_dir):
    if not os.path.isabs(stage_dir):
        die("Intel cleanup staging directory must be absolute")
    temp_parent = os.path.realpath(tempfile.gettempdir())
    if os.path.realpath(os.path.dirname(stage_dir)) != temp_parent:
        die("Intel cleanup staging directory must be directly under the temporary directory")
    name = os.path.basename(stage_dir)
    if not name.startswith("k2-intel-updater-"):
        die("Intel cleanup staging directory has the wrong prefix")
    return temp_parent, name


def close_fds(fds, error=None):
    for fd in fds:
        if fd is None:
            continue
        try:
            os.close(fd)
        except OSError as caught:
            if error is None:
                error = caught
    return error


def open_owned_stage(stage_dir, absent_ok, expected_mode):
    temp_parent, name = stage_location(stage_dir)
    parent_fd = os.open(
        temp_parent,
        os.O_RDONLY | getattr(os, "O_DIRECTORY", 0) | getattr(os, "O_NOFOLLOW", 0),
    )
    stage_fd = None
    try:
        try:
            before = os.stat(name, dir_fd=parent_fd, follow_symlinks=False)
        except FileNotFoundError:
            if absent_ok:
                os.close(parent_fd)
                return None, None, name
            raise
        if not stat.S_ISDIR(before.st_mode):
            die("Intel cleanup staging path must be a non-symlink directory")
        stage_fd = os.open(
            name,
            os.O_RDONLY | getattr(os, "O_DIRECTORY", 0) | getattr(os, "O_NOFOLLOW", 0),
            dir_fd=parent_fd,
        )
        opened = os.fstat(stage_fd)
        if (before.st_dev, before.st_ino) != (opened.st_dev, opened.st_ino):
            die("Intel cleanup staging directory changed while opening")
        if opened.st_uid != os.getuid() or stat.S_IMODE(opened.st_mode) != expected_mode:
            die("Intel cleanup staging directory has the wrong owner or mode")
        return parent_fd, stage_fd, name
    except BaseException as caught:
        error = close_fds((stage_fd, parent_fd), caught)
        raise error


def open_stage_entry(stage_fd, name, expected_mode):
    before = os.stat(name, dir_fd=stage_fd, follow_symlinks=False)
    fd = os.open(
        name,
        os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0),
        dir_fd=stage_fd,
    )
    opened = os.fstat(fd)
    if (before.st_dev, before.st_ino) != (opened.st_dev, opened.st_ino):
        os.close(fd)
        die(f"Intel cleanup {name} changed while opening")
    if not stat.S_ISREG(opened.st_mode) or opened.st_uid != os.getuid() \
            or opened.st_nlink != 1 or opened.st_size == 0 \
            or stat.S_IMODE(opened.st_mode) != expected_mode:
        os.close(fd)
        die(f"Intel cleanup {name} has the wrong type, owner, links, size, or mode")
    return fd, opened


def cleanup_stage(version, stage_dir, token):
    if TOKEN_RE.fullmatch(token) is None:
        die("Intel cleanup owner token must be 64 lowercase hexadecimal characters")
    name = f"K2_{version}_x86_64.app.tar.gz"
    expected = {OWNER_FILE, name, f"{name}.sig"}
    parent_fd = stage_fd = None
    entry_fds = []
    error = None
    try:
        parent_fd, stage_fd, stage_name = open_owned_stage(stage_dir, True, 0o555)
        if stage_fd is None:
            return
        if set(os.listdir(stage_fd)) != expected:
            die("Intel cleanup staging directory has missing or unexpected entries")
        opened = {}
        for entry in sorted(expected):
            fd, info = open_stage_entry(stage_fd, entry, 0o444)
            entry_fds.append(fd)
            opened[entry] = info
        if len({(info.st_dev, info.st_ino) for info in opened.values()}) != len(expected):
            die("Intel cleanup staging entries must be distinct files")
        marker = os.read(entry_fds[sorted(expected).index(OWNER_FILE)], 66)
        if marker != f"{token}\n".encode("ascii"):
            die("Intel cleanup owner token does not match")
        os.fchmod(stage_fd, 0o700)
        for entry in sorted(expected):
            current = os.stat(entry, dir_fd=stage_fd, follow_symlinks=False)
            original = opened[entry]
            if (current.st_dev, current.st_ino) != (original.st_dev, original.st_ino):
                die(f"Intel cleanup {entry} changed before unlink")
            os.unlink(entry, dir_fd=stage_fd)
        os.rmdir(stage_name, dir_fd=parent_fd)
    except BaseException as caught:
        error = caught
    finally:
        error = close_fds((*reversed(entry_fds), stage_fd, parent_fd), error)
    if error is not None:
        if isinstance(error, OSError):
            die(f"Intel cleanup operation failed (errno {error.errno})")
        raise error


def discard_partial_stage(stage_dir, token, archive_name):
    """Remove only the incomplete directory created by this stage call."""
    parent_fd = stage_fd = None
    error = None
    try:
        parent_fd, stage_fd, stage_name = open_owned_stage(stage_dir, False, 0o700)
        allowed = {OWNER_FILE, archive_name, f"{archive_name}.sig"}
        entries = set(os.listdir(stage_fd))
        if not entries.issubset(allowed) or OWNER_FILE not in entries:
            die("partial Intel staging directory has unexpected entries")
        marker_fd, _ = open_stage_entry(stage_fd, OWNER_FILE, 0o400)
        try:
            if os.read(marker_fd, 66) != f"{token}\n".encode("ascii"):
                die("partial Intel staging owner token does not match")
        except BaseException as caught:
            error = caught
        error = close_fds((marker_fd,), error)
        if error is not None:
            raise error
        for entry in entries - {OWNER_FILE}:
            fd = os.open(entry, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0), dir_fd=stage_fd)
            info = os.fstat(fd)
            os.close(fd)
            if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1:
                die("partial Intel staging entry is not owned regular data")
        for entry in sorted(entries):
            os.unlink(entry, dir_fd=stage_fd)
        os.rmdir(stage_name, dir_fd=parent_fd)
    except BaseException as caught:
        error = caught
    finally:
        error = close_fds((stage_fd, parent_fd), error)
    if error is not None:
        raise error


def validate_manifest(data, version):
    if not isinstance(data, dict) or data.get("version") != version:
        die(f"manifest version must equal {version}")
    platforms = data.get("platforms")
    if not isinstance(platforms, dict):
        die("manifest platforms must be an object")
    return platforms


def merge(path, version, archive, signature_path, url, config_path):
    validate_names(version, archive, signature_path, url)
    regular(archive, "archive", readonly=True)
    regular(signature_path, "signature", readonly=True)
    if os.stat(os.path.dirname(archive)).st_mode & 0o222:
        die("Intel staging directory must be read-only")
    verify_crypto(archive, signature_path, config_path)
    wrapped_signature, _ = signature_text(signature_path)
    data = load_json(path)
    platforms = validate_manifest(data, version)
    if "darwin-x86_64" in platforms:
        die("manifest already contains darwin-x86_64")
    platforms["darwin-x86_64"] = {"signature": wrapped_signature, "url": url}
    directory = os.path.dirname(os.path.abspath(path))
    fd, temp_path = tempfile.mkstemp(dir=directory, prefix=".latest-", text=True)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as output:
            json.dump(data, output, indent=2)
            output.write("\n")
        os.replace(temp_path, path)
    finally:
        if os.path.exists(temp_path):
            os.unlink(temp_path)


def verify(path, version, url, archive, signature_path, config_path):
    verify_crypto(archive, signature_path, config_path)
    expected_signature, _ = signature_text(signature_path)
    platforms = validate_manifest(load_json(path), version)
    entry = platforms.get("darwin-x86_64")
    if not isinstance(entry, dict) or entry.get("url") != url:
        die("manifest has no exact darwin-x86_64 URL")
    if entry.get("signature") != expected_signature:
        die("manifest darwin-x86_64 signature differs from the verified snapshot")


def main():
    command = sys.argv[1] if len(sys.argv) > 1 else ""
    args = sys.argv[2:]
    if command == "stage" and len(args) == 5:
        stage(*args)
    elif command == "cleanup" and len(args) == 3:
        cleanup_stage(*args)
    elif command == "merge" and len(args) == 6:
        merge(*args)
    elif command == "verify" and len(args) == 6:
        verify(*args)
    else:
        die("usage: intel-updater-manifest.py stage VERSION ARCHIVE SIGNATURE URL CONFIG | "
            "cleanup VERSION ABSOLUTE_STAGE_DIR OWNER_TOKEN | "
            "merge MANIFEST VERSION ARCHIVE SIGNATURE URL CONFIG | "
            "verify MANIFEST VERSION URL ARCHIVE SIGNATURE CONFIG")


if __name__ == "__main__":
    main()
