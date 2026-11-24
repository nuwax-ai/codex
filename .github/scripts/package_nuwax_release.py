#!/usr/bin/env python3
"""Package upstream-named Cargo outputs as the Nuwax Harness distribution."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import tarfile
import tempfile
import urllib.request
import zipfile


TARGETS = (
    "aarch64-apple-darwin", "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu", "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl", "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
)


# Bundled ripgrep: the engine only looks for `codex-path/rg` when the package
# is self-describing (`codex-package.json` beside the executable), and a release
# archive without it leaves `search` depending on a system rg that most hosts do
# not have. Upstream codex ships the same file for the same reason.
#
# Version and per-target asset come from BurntSushi/ripgrep. The aarch64 Linux
# *musl* target has no official musl build (only gnu), and a glibc-linked rg
# inside a static musl package would fail on musl-only hosts — worse than the
# system fallback — so that target ships no bundled rg.
RIPGREP_VERSION = "14.1.1"
RIPGREP_ASSETS = {
    "x86_64-unknown-linux-musl": "x86_64-unknown-linux-musl",
    "x86_64-unknown-linux-gnu": "x86_64-unknown-linux-musl",
    "x86_64-apple-darwin": "x86_64-apple-darwin",
    "aarch64-apple-darwin": "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc": "x86_64-pc-windows-msvc",
}


def _ripgrep_cache_root() -> Path:
    override = os.environ.get("NUWAX_RIPGREP_CACHE")
    if override:
        return Path(override)
    return Path.home() / ".cache" / "nuwax-ripgrep"


def bundled_ripgrep(target: str) -> Path | None:
    """The ripgrep binary to place at `codex-path/rg`, or None when the target
    has no suitable build. Downloads once into a cache directory."""
    asset = RIPGREP_ASSETS.get(target)
    if asset is None:
        return None
    windows = asset.endswith("-windows-msvc")
    name = f"rg{'.exe' if windows else ''}"
    cached = _ripgrep_cache_root() / f"{RIPGREP_VERSION}-{asset}" / name
    if cached.is_file() and cached.stat().st_size > 0:
        return cached
    cached.parent.mkdir(parents=True, exist_ok=True)
    suffix = ".zip" if windows else ".tar.gz"
    url = (
        "https://github.com/BurntSushi/ripgrep/releases/download/"
        f"{RIPGREP_VERSION}/ripgrep-{RIPGREP_VERSION}-{asset}{suffix}"
    )
    archive = cached.parent / f"download{suffix}"
    try:
        urllib.request.urlretrieve(url, archive)
        with tempfile.TemporaryDirectory(dir=cached.parent) as temp:
            if windows:
                with zipfile.ZipFile(archive) as bundle:
                    member = next(n for n in bundle.namelist() if n.endswith("/" + name))
                    bundle.extract(member, temp)
                    extracted = Path(temp, member)
            else:
                with tarfile.open(archive, "r:gz") as bundle:
                    member = next(m for m in bundle.getmembers() if m.name.endswith("/" + name))
                    bundle.extract(member, temp, filter="data")
                    extracted = Path(temp, member.name)
            shutil.copy2(extracted, cached)
        cached.chmod(0o755)
    except Exception as error:  # network, 404, unexpected layout
        raise FileNotFoundError(
            f"cannot fetch bundled ripgrep {RIPGREP_VERSION} for {asset} ({error}); "
            "put the binary at "
            f"{cached} (or set NUWAX_RIPGREP_CACHE) and re-run"
        ) from error
    finally:
        archive.unlink(missing_ok=True)
    if not cached.is_file() or cached.stat().st_size == 0:
        raise FileNotFoundError(f"bundled ripgrep extraction produced no binary at {cached}")
    return cached


def _sha256_of(binary) -> str:
    """Streaming SHA-256.

    `hashlib.file_digest` only exists on Python 3.11+, and release machines are
    not guaranteed to have that (the macOS CI runner ships 3.9). The chunked
    loop produces the same digest on every supported version.
    """
    digest = hashlib.sha256()
    for chunk in iter(lambda: binary.read(1024 * 1024), b""):
        digest.update(chunk)
    return digest.hexdigest()


def package(build_dir, output_dir, version, target, ripgrep=None, replace_existing=False):
    if target not in TARGETS:
        raise ValueError("unsupported release target")
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z][0-9A-Za-z.-]*)?", version):
        raise ValueError("invalid release version")
    windows = target.endswith("-windows-msvc")
    ext = ".exe" if windows else ""
    files = {
        "nuwax-harness" + ext: "codex" + ext,
        "codex-code-mode-host" + ext: "codex-code-mode-host" + ext,
    }
    # The legacy `nuwax-codex` executable alias lives only in the npm layer
    # (bin/nuwax-codex.js forwarding); archives ship a single main binary.
    if windows:
        files.update({name + ext: name + ext for name in (
            "codex-windows-sandbox-setup", "codex-command-runner",
        )})
    if "-linux-" in target:
        files["codex-resources/bwrap"] = "bwrap"
    if ripgrep is None:
        ripgrep = bundled_ripgrep(target)
    if ripgrep is not None:
        # Self-describing packages put it at `codex-path/rg`; the engine
        # prepends that directory to PATH for every command it runs.
        files[f"codex-path/rg{ext}"] = str(Path(ripgrep).resolve())
    build_dir, output_dir = Path(build_dir), Path(output_dir)
    for destination, source in files.items():
        candidate = Path(source) if Path(source).is_absolute() else build_dir / source
        if not candidate.is_file():
            raise FileNotFoundError(
                f"missing required release binary for {destination}: {candidate}"
            )
        if candidate.stat().st_size == 0:
            raise FileNotFoundError(f"empty release binary for {destination}: {candidate}")
    output_dir.mkdir(parents=True, exist_ok=True)
    suffix = ".zip" if windows else ".tar.gz"
    # The npm package and CDN namespace stay stable for existing installers.
    archive = output_dir / f"nuwax-codex-{version}-{target}{suffix}"
    if archive.exists():
        if not replace_existing:
            raise FileExistsError("release archive already exists; refusing stale artifact reuse")
        # Self-hosted CI keeps one artifact directory per runner across runs, so
        # an aborted attempt can leave a same-version archive behind and every
        # retry then fails on this guard. Dropping it first also keeps a failing
        # retry from leaving a half-trusted file for the next run to reuse.
        archive.unlink()
    with tempfile.TemporaryDirectory(prefix="nuwax-package-", dir=output_dir) as temp:
        stage = Path(temp) / "stage"
        stage.mkdir()
        sums = []
        # The package describes itself so the engine can find its helper
        # directories (`codex-path` for the bundled rg, `codex-resources` for
        # bwrap). Without it `InstallContext` has no `path_dir` and the bundled
        # ripgrep is invisible.
        metadata = {
            "layoutVersion": 1,
            "version": version,
            "target": target,
            "variant": "codex",
            "entrypoint": "nuwax-harness" + ext,
            "resourcesDir": "codex-resources",
            "pathDir": "codex-path",
        }
        (stage / "codex-package.json").write_text(
            json.dumps(metadata, indent=2) + "\n", encoding="utf-8"
        )
        for name, source in files.items():
            destination = stage / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            origin = Path(source) if Path(source).is_absolute() else build_dir / source
            try:
                os.link(origin, destination)
            except OSError:
                shutil.copy2(origin, destination)
            with destination.open("rb") as binary:
                digest = _sha256_of(binary)
            sums.append(f"{digest}  {name}\n")
        (stage / "SHA256SUMS").write_text("".join(sums), encoding="utf-8")
        names = [*files, "codex-package.json", "SHA256SUMS"]
        temporary_archive = Path(temp) / ("archive" + suffix)
        if windows:
            with zipfile.ZipFile(temporary_archive, "w", compression=zipfile.ZIP_DEFLATED) as output:
                for name in names:
                    output.write(stage / name, name)
        else:
            with tarfile.open(temporary_archive, "w:gz") as output:
                for name in names:
                    output.add(stage / name, arcname=name, recursive=False)
        temporary_archive.rename(archive)
    return archive


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build-dir", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--target", required=True, choices=TARGETS)
    parser.add_argument("--github-output", type=Path)
    parser.add_argument(
        "--ripgrep",
        type=Path,
        default=None,
        help=(
            "ripgrep binary to bundle at codex-path/rg; defaults to a cached "
            "download for the targets that have an official build"
        ),
    )
    parser.add_argument(
        "--replace-existing",
        action="store_true",
        help=(
            "replace a same-version archive left by an earlier run; only for "
            "self-hosted CI where the artifact directory survives between runs"
        ),
    )
    args = parser.parse_args()
    archive = package(
        args.build_dir,
        args.output_dir,
        args.version,
        args.target,
        ripgrep=args.ripgrep,
        replace_existing=args.replace_existing,
    )
    if args.github_output:
        with args.github_output.open("a", encoding="utf-8") as output:
            output.write(f"archive_path={archive.name}\narchive_name={archive.name}\n")
    print(json.dumps({"archive": str(archive), "target": args.target}))


if __name__ == "__main__":
    main()
