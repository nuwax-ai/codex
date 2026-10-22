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
import zipfile


TARGETS = (
    "aarch64-apple-darwin", "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu", "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl", "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
)


def package(build_dir, output_dir, version, target):
    if target not in TARGETS:
        raise ValueError("unsupported release target")
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z][0-9A-Za-z.-]*)?", version):
        raise ValueError("invalid release version")
    windows = target.endswith("-windows-msvc")
    ext = ".exe" if windows else ""
    files = {
        "nuwax-harness" + ext: "codex" + ext,
        # Existing ACP packages resolve this filename directly. It is an
        # alias of the new main executable, not a separately built product.
        "nuwax-codex" + ext: "codex" + ext,
        "codex-code-mode-host" + ext: "codex-code-mode-host" + ext,
    }
    if windows:
        files.update({name + ext: name + ext for name in (
            "codex-windows-sandbox-setup", "codex-command-runner",
        )})
    if "-linux-" in target:
        files["codex-resources/bwrap"] = "bwrap"
    build_dir, output_dir = Path(build_dir), Path(output_dir)
    for source in files.values():
        if not (build_dir / source).is_file():
            raise FileNotFoundError(f"missing required release binary: {source}")
    output_dir.mkdir(parents=True, exist_ok=True)
    suffix = ".zip" if windows else ".tar.gz"
    # The npm package and CDN namespace stay stable for existing installers.
    archive = output_dir / f"nuwax-codex-{version}-{target}{suffix}"
    if archive.exists():
        raise FileExistsError("release archive already exists; refusing stale artifact reuse")
    with tempfile.TemporaryDirectory(prefix="nuwax-package-", dir=output_dir) as temp:
        stage = Path(temp) / "stage"
        stage.mkdir()
        sums = []
        for name, source in files.items():
            destination = stage / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            # A tar hardlink keeps the legacy alias without doubling its size.
            try:
                os.link(build_dir / source, destination)
            except OSError:
                shutil.copy2(build_dir / source, destination)
            with destination.open("rb") as binary:
                digest = hashlib.file_digest(binary, "sha256").hexdigest()
            sums.append(f"{digest}  {name}\n")
        (stage / "SHA256SUMS").write_text("".join(sums), encoding="utf-8")
        names = [*files, "SHA256SUMS"]
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
    args = parser.parse_args()
    archive = package(args.build_dir, args.output_dir, args.version, args.target)
    if args.github_output:
        with args.github_output.open("a", encoding="utf-8") as output:
            output.write(f"archive_path={archive.name}\narchive_name={archive.name}\n")
    print(json.dumps({"archive": str(archive), "target": args.target}))


if __name__ == "__main__":
    main()
