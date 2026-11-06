#!/usr/bin/env python3
"""Release manifest for the Gitea CI.

Each platform job writes one *entry* (file name, url, sha256, size) next to its
archive and uploads both. The verification job downloads the entries, assembles
``manifest.json`` and uploads it, so release tooling and the npm launcher can
read a single file instead of hard-coding seven URLs.

    release-manifest.py entry --archive A --version V --target T --base-url U --out F
    release-manifest.py assemble --entries DIR --version V --tag TAG \
        --base-url U --repository R --commit C --out F

Only the standard library is used, and the code sticks to features that exist on
Python 3.9 (the macOS runner), so the same file works on every host.
"""

import argparse
import datetime
import hashlib
import json
import os
import sys

CHUNK = 1 << 20


def sha256_of(path):
    digest = hashlib.sha256()
    with open(path, "rb") as binary:
        for chunk in iter(lambda: binary.read(CHUNK), b""):
            digest.update(chunk)
    return digest.hexdigest()


def write_json(path, payload):
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(payload, handle, indent=2, sort_keys=True)
        handle.write("\n")


def cmd_entry(args):
    archive = os.path.abspath(args.archive)
    if not os.path.isfile(archive):
        print(f"no archive at {archive}", file=sys.stderr)
        return 1
    name = os.path.basename(archive)
    entry = {
        "target": args.target,
        "file": name,
        "url": f"{args.base_url.rstrip('/')}/{name}",
        "sha256": sha256_of(archive),
        "size": os.path.getsize(archive),
    }
    write_json(args.out, entry)
    print(json.dumps(entry, indent=2, sort_keys=True))
    return 0


def cmd_assemble(args):
    targets = {}
    for name in sorted(os.listdir(args.entries)):
        if not name.endswith(".json") or name == "manifest.json":
            continue
        with open(os.path.join(args.entries, name), encoding="utf-8") as handle:
            entry = json.load(handle)
        targets[entry["target"]] = entry

    manifest = {
        "version": args.version,
        "tag": args.tag,
        "generated_at": datetime.datetime.now(datetime.timezone.utc).strftime(
            "%Y-%m-%dT%H:%M:%SZ"
        ),
        "repository": args.repository,
        "commit": args.commit,
        "base_url": args.base_url.rstrip("/"),
        "targets": targets,
    }
    write_json(args.out, manifest)
    print(json.dumps(manifest, indent=2, sort_keys=True))
    return 0


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    entry = sub.add_parser("entry", help="write one platform entry")
    entry.add_argument("--archive", required=True)
    entry.add_argument("--version", required=True)
    entry.add_argument("--target", required=True)
    entry.add_argument("--base-url", required=True)
    entry.add_argument("--out", required=True)
    entry.set_defaults(func=cmd_entry)

    assemble = sub.add_parser("assemble", help="merge entries into manifest.json")
    assemble.add_argument("--entries", required=True)
    assemble.add_argument("--version", required=True)
    assemble.add_argument("--tag", required=True)
    assemble.add_argument("--base-url", required=True)
    assemble.add_argument("--repository", default="")
    assemble.add_argument("--commit", default="")
    assemble.add_argument("--out", required=True)
    assemble.set_defaults(func=cmd_assemble)

    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
