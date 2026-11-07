#!/usr/bin/env python3
"""Builds and checks the webOS app package (.ipk) of webos-satellite.

Standard library only. The output is reproducible: for the same inputs and
SOURCE_DATE_EPOCH it is byte for byte the same (sorted entries, fixed
mtimes, uid/gid 0, gzip without name or time).

Layout follows what LG's packager (ares-package, webos-tools/cli
lib/package.js) writes and what appinstalld and the Homebrew Channel read:
an `ar` archive with `debian-binary` ("2.0"), `control.tar.gz` (one
`control` file) and `data.tar.gz` holding
`usr/palm/applications/<id>/...` and `usr/palm/packages/<id>/packageinfo.json`.
Design: docs/design/packaging.md.

usage:
  build_ipk.py build --armv7 BIN --aarch64 BIN
                     [--setup-armv7 BIN --setup-aarch64 BIN]
                     [--proxy-armv7 BIN --proxy-aarch64 BIN]
                     [--app-id ID] [--version V] [--out DIR] [--no-elf-check]
  build_ipk.py check IPK [--app-id ID] [--version V] [--no-elf-check]
                     [--require-setup] [--require-proxy]

The phone setup server (webos-satellite-setup, ADR 0009) goes in as two more
builds, both or neither; so does the remote's input grab helper
(remote-proxy, ADR 0016).

The assistant overlay (docs/design/assistant-overlay.md, ADR 0014) is a second
web app, `<id>.overlay`, with an `overlay` window. It is built as its own
single-app ipk and shipped as payload/overlay.ipk: SAM does not register a
second app of one package until it restarts (EXP-024, c3-a), so `ctl.sh
install` hands that ipk to appinstalld itself. `check` accepts a package with both or with none;
--require-setup (a release) refuses one without.
"""

from __future__ import annotations

import argparse
import base64
import gzip
import hashlib
import io
import json
import os
import re
import struct
import subprocess
import sys
import tarfile
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent
PKG = HERE.parent  # packaging/webos
ROOT = PKG.parent.parent
DEFAULT_APP_ID = "io.github.pando85.voicesatellite"
TITLE = "Voice Satellite"
DESCRIPTION = "Home Assistant voice satellite for rooted LG webOS TVs (ESPHome native API)"
SOURCE_URL = "https://github.com/pando85/webos-voice-satellite"
LICENSE_ID = "GPL-3.0-or-later"  # SPDX; keep equal to workspace.package.license in Cargo.toml

# Files of packaging/webos/ shipped in payload/ (scripts run with `sh`).
PAYLOAD_SCRIPTS = [
    "autostart.sh", "ctl.sh", "install.sh", "lib.sh", "rotate.sh", "start.sh",
    "status.sh", "stop.sh", "supervise.sh", "uninstall.sh",
]
APP_FILES = ["index.html", "app.js", "voicepower.js", "style.css", "qrcodegen.js", "QRCODEGEN-LICENSE.txt"]
# The overlay app (packaging/webos/overlay/), its id suffix and window.
OVERLAY_FILES = ["index.html", "model.js", "overlay.js", "overlay.css"]
OVERLAY_SUFFIX = ".overlay"
OVERLAY_TITLE = "Voice Satellite overlay"
ID_RE = re.compile(r"^[a-z0-9][a-z0-9.+-]+$")  # LG appinfo.json rules; ares-cli's check
VERSION_RE = re.compile(r"^(0|[1-9]\d{0,8})\.(0|[1-9]\d{0,8})\.(0|[1-9]\d{0,8})$")
FORBIDDEN_PREFIXES = ("com.palm", "com.webos", "com.lge", "com.palmdts")
ELF_MACHINE = {"armv7": (1, 40), "aarch64": (2, 183)}  # (EI_CLASS, e_machine)
SETUP_NAMES = [f"webos-satellite-setup-{a}" for a in ELF_MACHINE]
PROXY_NAMES = [f"remote-proxy-{a}" for a in ELF_MACHINE]


def die(msg: str) -> None:
    print(f"build_ipk: {msg}", file=sys.stderr)
    sys.exit(1)


def workspace_version() -> str:
    import tomllib

    with open(ROOT / "Cargo.toml", "rb") as f:
        return tomllib.load(f)["workspace"]["package"]["version"]


def source_date_epoch() -> int:
    env = os.environ.get("SOURCE_DATE_EPOCH")
    if env:
        return int(env)
    try:
        out = subprocess.run(["git", "-C", str(ROOT), "log", "-1", "--format=%ct"],
                             capture_output=True, text=True, check=True).stdout.strip()
        return int(out)
    except (OSError, subprocess.CalledProcessError, ValueError):
        return 0


# --- icon ------------------------------------------------------------------

def png(width: int, height: int, rows: list[bytes]) -> bytes:
    def chunk(kind: bytes, data: bytes) -> bytes:
        return (struct.pack(">I", len(data)) + kind + data
                + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF))

    raw = b"".join(b"\x00" + r for r in rows)
    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def icon(size: int) -> bytes:
    """A microphone on a rounded blue square, drawn procedurally (RGBA)."""
    bg, fg = (31, 111, 178, 255), (240, 245, 250, 255)
    s = size / 80.0
    rows = []
    for y in range(size):
        row = bytearray()
        for x in range(size):
            px, py = (x + 0.5) / s, (y + 0.5) / s  # 80x80 design space
            # rounded square, radius 16
            dx = max(16 - px, 0, px - 64)
            dy = max(16 - py, 0, py - 64)
            inside = dx * dx + dy * dy <= 16 * 16
            mic = (abs(px - 40) <= 9 and 14 <= py <= 44 and
                   (py >= 23 or (px - 40) ** 2 + (py - 23) ** 2 <= 81) and
                   (py <= 35 or (px - 40) ** 2 + (py - 35) ** 2 <= 81))
            r = ((px - 40) ** 2 + (py - 36) ** 2) ** 0.5
            arc = 14.5 <= r <= 18 and py >= 36
            stand = abs(px - 40) <= 1.8 and 52 <= py <= 62
            base = abs(px - 40) <= 10 and 60 <= py <= 63
            if not inside:
                row += bytes((0, 0, 0, 0))
            elif mic or arc or stand or base:
                row += bytes(fg)
            else:
                row += bytes(bg)
        rows.append(bytes(row))
    return png(size, size, rows)


# --- archive writers -------------------------------------------------------

def tar_gz(entries: list[tuple[str, bytes | None, int]], mtime: int) -> bytes:
    """entries: (path, content or None for a directory, mode)."""
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w", format=tarfile.USTAR_FORMAT) as tar:
        for name, data, mode in sorted(entries, key=lambda e: e[0]):
            info = tarfile.TarInfo(name)
            info.mtime = mtime
            info.uid = info.gid = 0
            info.uname = info.gname = "root"
            info.mode = mode
            if data is None:
                info.type = tarfile.DIRTYPE
                tar.addfile(info)
            else:
                info.size = len(data)
                tar.addfile(info, io.BytesIO(data))
    out = io.BytesIO()
    with gzip.GzipFile(filename="", mode="wb", fileobj=out, mtime=0, compresslevel=9) as gz:
        gz.write(buf.getvalue())
    return out.getvalue()


def ar(members: list[tuple[str, bytes]], mtime: int) -> bytes:
    out = bytearray(b"!<arch>\n")
    for name, data in members:
        header = (f"{name:<16}{mtime:<12}{0:<6}{0:<6}{'100644':<8}{len(data):<10}`\n").encode()
        assert len(header) == 60
        out += header + data
        if len(data) % 2:
            out += b"\n"
    return bytes(out)


def ar_members(blob: bytes) -> list[tuple[str, bytes]]:
    if not blob.startswith(b"!<arch>\n"):
        raise ValueError("not an ar archive")
    pos, members = 8, []
    while pos < len(blob):
        header = blob[pos:pos + 60]
        if len(header) < 60 or header[58:60] != b"`\n":
            raise ValueError(f"bad ar header at {pos}")
        name = header[:16].decode().strip().rstrip("/")
        size = int(header[48:58].decode().strip())
        members.append((name, blob[pos + 60:pos + 60 + size]))
        pos += 60 + size + (size % 2)
    return members


# --- build -----------------------------------------------------------------

def elf_check(path: Path, arch: str) -> None:
    head = path.read_bytes()[:20]
    want_class, want_machine = ELF_MACHINE[arch]
    if head[:4] != b"\x7fELF" or head[4] != want_class or struct.unpack("<H", head[18:20])[0] != want_machine:
        die(f"{path} is not an {arch} ELF executable")


def package(app_id: str, version: str, app: dict[str, tuple[bytes, int]], mtime: int, description: str) -> bytes:
    """One single-app ipk: `app` maps file names in the app directory
    (`payload/...` allowed) to (data, mode)."""
    appdir = f"usr/palm/applications/{app_id}"
    pkgdir = f"usr/palm/packages/{app_id}"
    entries: list[tuple[str, bytes | None, int]] = []
    dirs = ["usr", "usr/palm", "usr/palm/applications", appdir]
    if any(n.startswith("payload/") for n in app):
        dirs.append(f"{appdir}/payload")
    for d in dirs + ["usr/palm/packages", pkgdir]:
        entries.append((d + "/", None, 0o755))
    for name, (data, mode) in app.items():
        entries.append((f"{appdir}/{name}", data, mode))
    packageinfo = {"app": app_id, "id": app_id, "version": version}
    entries.append((f"{pkgdir}/packageinfo.json",
                    (json.dumps(packageinfo, indent=2) + "\n").encode(), 0o644))
    for name, _, _ in entries:
        if len(name) > 99:
            die(f"path too long for a plain ustar name: {name}")

    installed_size = sum(len(d) for _, d, _ in entries if d is not None)
    control = (f"Package: {app_id}\nVersion: {version}\nSection: misc\nPriority: optional\n"
               f"Architecture: all\nInstalled-Size: {installed_size}\n"
               f"Maintainer: N/A <nobody@example.com>\nLicense: {LICENSE_ID}\nDescription: {description}\n"
               f"webOS-Package-Format-Version: 2\nwebOS-Packager-Version: x.y.x\n")
    return ar([("debian-binary", b"2.0\n"),
               ("control.tar.gz", tar_gz([("control", control.encode(), 0o644)], mtime)),
               ("data.tar.gz", tar_gz(entries, mtime))], mtime)


def overlay_package(app_id: str, version: str, mtime: int) -> bytes:
    """The overlay app's ipk (id `<app_id>.overlay`)."""
    ov_id = app_id + OVERLAY_SUFFIX
    appinfo = {
        "id": ov_id,
        "version": version,
        "vendor": "webos-voice-satellite",
        "type": "web",
        "main": "index.html",
        "title": OVERLAY_TITLE,
        "appDescription": "On-screen assistant of " + TITLE + " (started by the satellite)",
        "icon": "icon.png",
        "iconColor": "#1f6fb2",
        # EXP-024 (c3-a, webOS 8.3): "overlay" starts as a window over the app
        # in front (that app stays the foreground app); "topui" is refused
        # ("Unsupported Application Type").
        "defaultWindowType": "overlay",
        "transparent": True,
        "noSplashOnLaunch": True,
        "spinnerOnLaunch": False,
        # Not in the launcher, not in Recents.
        "visible": False,
        "class": {"hidden": True},
        "disableBackHistoryAPI": True,
    }
    app: dict[str, tuple[bytes, int]] = {
        "appinfo.json": ((json.dumps(appinfo, indent=2) + "\n").encode(), 0o644),
        "icon.png": (icon(80), 0o644),
    }
    for name in OVERLAY_FILES:
        app[name] = ((PKG / "overlay" / name).read_bytes(), 0o644)
    app["LICENSE"] = ((ROOT / "LICENSE").read_bytes(), 0o644)
    return package(ov_id, version, app, mtime, "On-screen assistant overlay of webos-satellite")


def build(args: argparse.Namespace) -> Path:
    app_id, version = args.app_id, args.version or workspace_version()
    check_id_version(app_id, version)
    mtime = source_date_epoch()
    bins = {"armv7": Path(args.armv7), "aarch64": Path(args.aarch64)}
    for arch, path in bins.items():
        if not path.is_file():
            die(f"missing {arch} binary: {path}")
        if not args.no_elf_check:
            elf_check(path, arch)
    setup_bins = {}
    if args.setup_armv7 or args.setup_aarch64:
        if not (args.setup_armv7 and args.setup_aarch64):
            die("give both --setup-armv7 and --setup-aarch64, or neither")
        setup_bins = {"armv7": Path(args.setup_armv7), "aarch64": Path(args.setup_aarch64)}
        for arch, path in setup_bins.items():
            if not path.is_file():
                die(f"missing {arch} setup server binary: {path}")
            if not args.no_elf_check:
                elf_check(path, arch)

    proxy_bins = {}
    if args.proxy_armv7 or args.proxy_aarch64:
        if not (args.proxy_armv7 and args.proxy_aarch64):
            die("give both --proxy-armv7 and --proxy-aarch64, or neither")
        proxy_bins = {"armv7": Path(args.proxy_armv7), "aarch64": Path(args.proxy_aarch64)}
        for arch, path in proxy_bins.items():
            if not path.is_file():
                die(f"missing {arch} remote grab helper binary: {path}")
            if not args.no_elf_check:
                elf_check(path, arch)

    payload: dict[str, tuple[bytes, int]] = {}
    for name in PAYLOAD_SCRIPTS:
        payload[name] = ((PKG / name).read_bytes(), 0o755)
    payload["webos-satellite.hook"] = ((PKG / "init.d" / "webos-satellite").read_bytes(), 0o644)
    payload["config.default.toml"] = ((PKG / "config.default.toml").read_bytes(), 0o644)
    for arch, path in bins.items():
        payload[f"webos-satellite-{arch}"] = (path.read_bytes(), 0o755)
    for arch, path in setup_bins.items():
        payload[f"webos-satellite-setup-{arch}"] = (path.read_bytes(), 0o755)
    for arch, path in proxy_bins.items():
        payload[f"remote-proxy-{arch}"] = (path.read_bytes(), 0o755)
    payload["VERSION"] = (f"{version}\n".encode(), 0o644)
    payload["overlay.ipk"] = (overlay_package(app_id, version, mtime), 0o644)
    sums = "".join(f"{hashlib.sha256(data).hexdigest()}  {name}\n"
                   for name, (data, _) in sorted(payload.items()))
    payload["SHA256SUMS"] = (sums.encode(), 0o644)

    appinfo = {
        "id": app_id,
        "version": version,
        "vendor": "webos-voice-satellite",
        "type": "web",
        "main": "index.html",
        "title": TITLE,
        "appDescription": DESCRIPTION,
        "icon": "icon.png",
        "largeIcon": "largeIcon.png",
        "iconColor": "#1f6fb2",
        # The app handles the remote's Back key itself (keyCode 461); without this
        # the runtime does history.back() and closes the app from any screen.
        # https://webostv.developer.lge.com/develop/guides/back-button
        "disableBackHistoryAPI": True,
    }
    app: dict[str, tuple[bytes, int]] = {
        "appinfo.json": ((json.dumps(appinfo, indent=2) + "\n").encode(), 0o644),
        "icon.png": (icon(80), 0o644),
        "largeIcon.png": (icon(130), 0o644),
    }
    for name in APP_FILES:
        app[name] = ((PKG / "app" / name).read_bytes(), 0o644)
    app["LICENSE"] = ((ROOT / "LICENSE").read_bytes(), 0o644)  # the GPL text travels with the binaries
    for name, entry in payload.items():
        app[f"payload/{name}"] = entry

    blob = package(app_id, version, app, mtime, DESCRIPTION)

    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    ipk_name = f"{app_id}_{version}_all.ipk"
    ipk = out / ipk_name
    ipk.write_bytes(blob)
    digest = hashlib.sha256(blob).hexdigest()
    # Homebrew Channel package manifest (webosbrew/docs schema), for a later
    # self-hosted repository; ipkUrl is relative to the manifest's URL.
    manifest = {
        "id": app_id, "version": version, "type": "web", "title": TITLE,
        "appDescription": DESCRIPTION,
        "iconUri": "data:image/png;base64," + base64.b64encode(icon(130)).decode(),
        "sourceUrl": SOURCE_URL, "rootRequired": True,
        "ipkUrl": ipk_name, "ipkHash": {"sha256": digest},
    }
    (out / f"{app_id}.manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    (out / f"{ipk_name}.sha256").write_text(f"{digest}  {ipk_name}\n")
    print(f"{ipk}\nsha256 {digest}\nsize {len(blob)} bytes")
    return ipk


# --- check -----------------------------------------------------------------

def check_id_version(app_id: str, version: str) -> None:
    if not ID_RE.match(app_id) or app_id.startswith(FORBIDDEN_PREFIXES):
        die(f"invalid app id {app_id!r} (lowercase letters, digits, '.', '+', '-'; not com.palm/com.webos/com.lge)")
    if not VERSION_RE.match(version):
        die(f"invalid version {version!r}: three numbers without leading zeros, as webOS requires")


def untar(blob: bytes) -> dict[str, tuple[tarfile.TarInfo, bytes | None]]:
    out = {}
    with tarfile.open(fileobj=io.BytesIO(gzip.decompress(blob)), mode="r:") as tar:
        for info in tar.getmembers():
            data = tar.extractfile(info).read() if info.isfile() else None
            out[info.name] = (info, data)
    return out


def check_overlay(blob: bytes, ov_id: str, version: str) -> list[str]:
    """Problems of the embedded overlay ipk (empty when it is fine)."""
    errors: list[str] = []
    try:
        m = dict(ar_members(blob))
        ctrl = untar(m["control.tar.gz"])
        fields = dict(line.split(": ", 1) for line in ctrl["control"][1].decode().splitlines() if line)
        data = untar(m["data.tar.gz"])
    except Exception as e:  # noqa: BLE001 - any parse failure is a check failure
        return [f"not an ipk ({e.__class__.__name__})"]
    if fields.get("Package") != ov_id or fields.get("Version") != version:
        errors.append(f"Package/Version {fields.get('Package')} {fields.get('Version')}")
    appdir = f"usr/palm/applications/{ov_id}"
    apps = {n.split("/")[3] for n in data if n.startswith("usr/palm/applications/") and n.count("/") >= 3}
    if apps != {ov_id}:
        errors.append(f"application directories {sorted(apps)}")
        return errors
    for name, (info, _) in data.items():
        if name.startswith("/") or ".." in name.split("/") or info.uid != 0 or info.mode & 0o022:
            errors.append(f"unsafe entry {name}")
    appinfo = json.loads(data[f"{appdir}/appinfo.json"][1])
    want = {"id": ov_id, "version": version, "type": "web", "main": "index.html",
            "defaultWindowType": "overlay", "transparent": True, "noSplashOnLaunch": True,
            "spinnerOnLaunch": False, "disableBackHistoryAPI": True}
    for k, v in want.items():
        if appinfo.get(k) != v:
            errors.append(f"appinfo {k} is {appinfo.get(k)!r}, not {v!r}")
    for name in OVERLAY_FILES + ["icon.png", "LICENSE"]:
        if f"{appdir}/{name}" not in data:
            errors.append(f"lacks {name}")
    return errors


def check(args: argparse.Namespace) -> None:
    errors: list[str] = []

    def need(cond: bool, msg: str) -> None:
        if not cond:
            errors.append(msg)

    path = Path(args.ipk)
    members = ar_members(path.read_bytes())
    names = [n for n, _ in members]
    need(names == ["debian-binary", "control.tar.gz", "data.tar.gz"], f"ar members {names}")
    m = dict(members)
    need(m.get("debian-binary") == b"2.0\n", "debian-binary is not 2.0")
    ctrl = untar(m["control.tar.gz"])
    need(list(ctrl) == ["control"], f"control.tar.gz holds {list(ctrl)}")
    fields = dict(line.split(": ", 1) for line in ctrl["control"][1].decode().splitlines() if line)
    for key in ["Package", "Version", "Section", "Priority", "Architecture", "Installed-Size",
                "Maintainer", "License", "Description", "webOS-Package-Format-Version"]:
        need(key in fields, f"control lacks {key}")
    app_id, version = fields.get("Package", ""), fields.get("Version", "")
    check_id_version(app_id, version)
    need(not args.app_id or app_id == args.app_id, f"Package {app_id} != {args.app_id}")
    need(not args.version or version == args.version, f"Version {version} != {args.version}")
    need(fields.get("Architecture") == "all", "Architecture is not all")
    need(fields.get("License") == LICENSE_ID, f"License {fields.get('License')} != {LICENSE_ID}")
    need(path.name == f"{app_id}_{version}_all.ipk", f"file name {path.name}")

    data = untar(m["data.tar.gz"])
    appdir = f"usr/palm/applications/{app_id}"
    pkgdir = f"usr/palm/packages/{app_id}"
    for name, (info, _) in data.items():
        need(name.startswith(("usr/palm/applications/", "usr/palm/packages/"))
             or name in ("usr", "usr/palm", "usr/palm/applications", "usr/palm/packages"),
             f"unexpected path {name}")
        need(not name.startswith("/") and ".." not in name.split("/"), f"unsafe path {name}")
        need(info.isfile() or info.isdir(), f"not a file or directory: {name}")
        need(info.uid == 0 and info.gid == 0, f"owner not root: {name}")
        need(info.mode & 0o022 == 0, f"group/other writable: {name}")
        need(info.mode & 0o004 or info.isdir(), f"not world-readable: {name}")
    need(set(n.split("/")[3] for n in data if n.startswith("usr/palm/applications/") and n.count("/") >= 3)
         == {app_id}, "more than one application directory")

    appinfo = json.loads(data[f"{appdir}/appinfo.json"][1])
    for key in ["id", "version", "type", "main", "title", "icon"]:
        need(key in appinfo, f"appinfo.json lacks {key}")
    need(appinfo.get("id") == app_id and appinfo.get("version") == version, "appinfo id/version differ from control")
    need(appinfo.get("type") == "web", "appinfo type is not web")
    need(appinfo.get("disableBackHistoryAPI") is True, "appinfo.json lacks disableBackHistoryAPI: true (Back would close the app)")
    for key in ["main", "icon", "largeIcon"]:
        if key in appinfo:
            need(f"{appdir}/{appinfo[key]}" in data, f"appinfo {key} {appinfo[key]} missing")
    for key, size in [("icon", 80), ("largeIcon", 130)]:
        png_data = data.get(f"{appdir}/{appinfo.get(key)}", (None, b""))[1] or b""
        need(png_data[:8] == b"\x89PNG\r\n\x1a\n" and struct.unpack(">II", png_data[16:24]) == (size, size),
             f"{key} is not a {size}x{size} PNG")
    need(f"{appdir}/LICENSE" in data and b"GNU GENERAL PUBLIC LICENSE" in (data[f"{appdir}/LICENSE"][1] or b""),
         "the app directory lacks the GPL LICENSE text")
    for name in APP_FILES:
        need(f"{appdir}/{name}" in data, f"the app directory lacks {name}")
    need(b"Project Nayuki" in (data[f"{appdir}/qrcodegen.js"][1] or b"")
         and b"MIT License" in (data[f"{appdir}/QRCODEGEN-LICENSE.txt"][1] or b""),
         "the QR generator lacks its copyright and MIT notice")
    pkginfo = json.loads(data[f"{pkgdir}/packageinfo.json"][1])
    need(pkginfo == {"app": app_id, "id": app_id, "version": version}, f"packageinfo.json {pkginfo}")

    payload = {n[len(appdir) + 9:]: d for n, (_, d) in data.items()
               if n.startswith(f"{appdir}/payload/") and d is not None}
    expected = set(PAYLOAD_SCRIPTS) | {"webos-satellite.hook", "config.default.toml", "VERSION",
                                       "SHA256SUMS", "webos-satellite-armv7", "webos-satellite-aarch64",
                                       "overlay.ipk"}
    have_setup = [n for n in SETUP_NAMES if n in payload]
    need(len(have_setup) in (0, len(SETUP_NAMES)), f"only one build of the setup server: {have_setup}")
    need(have_setup or not args.require_setup, "the phone setup server is not in the package (--require-setup)")
    if have_setup:
        expected |= set(SETUP_NAMES)
    have_proxy = [n for n in PROXY_NAMES if n in payload]
    need(len(have_proxy) in (0, len(PROXY_NAMES)), f"only one build of the remote grab helper: {have_proxy}")
    need(have_proxy or not args.require_proxy, "the remote grab helper is not in the package (--require-proxy)")
    if have_proxy:
        expected |= set(PROXY_NAMES)
    need(set(payload) == expected, f"payload files differ: {sorted(set(payload) ^ expected)}")
    need(payload.get("VERSION") == f"{version}\n".encode(), "payload VERSION differs")
    for line in (payload.get("SHA256SUMS") or b"").decode().splitlines():
        digest, name = line.split("  ", 1)
        need(hashlib.sha256(payload.get(name, b"")).hexdigest() == digest, f"SHA256SUMS mismatch for {name}")
    need(len((payload.get("SHA256SUMS") or b"").decode().splitlines()) == len(expected) - 1,
         "SHA256SUMS does not cover every payload file")
    for name in PAYLOAD_SCRIPTS + ["webos-satellite-armv7", "webos-satellite-aarch64"] + have_setup + have_proxy:
        info = data.get(f"{appdir}/payload/{name}", (None, None))[0]
        need(info is not None and info.mode == 0o755, f"payload/{name} mode is not 0755")
    for name in PAYLOAD_SCRIPTS:
        if name != "lib.sh":  # sourced, not run
            need((payload.get(name) or b"").startswith(b"#!/bin/sh"), f"payload/{name} is not a sh script")
    if not args.no_elf_check:
        for arch in ELF_MACHINE:
            for name in ([f"webos-satellite-{arch}"] + ([f"webos-satellite-setup-{arch}"] if have_setup else [])
                         + ([f"remote-proxy-{arch}"] if have_proxy else [])):
                blob = payload.get(name) or b""
                want_class, want_machine = ELF_MACHINE[arch]
                need(blob[:4] == b"\x7fELF" and blob[4] == want_class
                     and struct.unpack("<H", blob[18:20])[0] == want_machine, f"{name} is not {arch} ELF")
    need(b"@PREFIX@" in (payload.get("webos-satellite.hook") or b""), "hook template lacks @PREFIX@")
    for e in check_overlay(payload.get("overlay.ipk") or b"", app_id + OVERLAY_SUFFIX, version):
        errors.append(f"payload/overlay.ipk: {e}")
    secret = re.compile(rb"^\s*noise_psk\s*=", re.M)
    need(not secret.search(payload.get("config.default.toml") or b""), "default config holds an inline key")

    if errors:
        print("\n".join(f"FAIL  {e}" for e in errors), file=sys.stderr)
        sys.exit(1)
    print(f"ok: {path.name} ({len(data)} entries, payload {len(payload)} files"
          f"{', with the setup server' if have_setup else ', no setup server'}"
          f"{', with the remote grab helper' if have_proxy else ', no remote grab helper'}, overlay app {app_id}{OVERLAY_SUFFIX})")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    b = sub.add_parser("build")
    b.add_argument("--armv7", required=True)
    b.add_argument("--aarch64", required=True)
    b.add_argument("--setup-armv7")
    b.add_argument("--setup-aarch64")
    b.add_argument("--proxy-armv7")
    b.add_argument("--proxy-aarch64")
    b.add_argument("--app-id", default=DEFAULT_APP_ID)
    b.add_argument("--version")
    b.add_argument("--out", default=str(ROOT / "target" / "ipk"))
    b.add_argument("--no-elf-check", action="store_true", help="accept stand-in binaries (tests)")
    c = sub.add_parser("check")
    c.add_argument("ipk")
    c.add_argument("--app-id")
    c.add_argument("--version")
    c.add_argument("--no-elf-check", action="store_true")
    c.add_argument("--require-setup", action="store_true", help="refuse a package without the setup server")
    c.add_argument("--require-proxy", action="store_true", help="refuse a package without the remote grab helper")
    args = ap.parse_args()
    if args.cmd == "build":
        build(args)
    else:
        check(args)


if __name__ == "__main__":
    main()
