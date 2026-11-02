#!/usr/bin/env python3
"""Refresh the bundled vendor model catalog from models.dev.

Usage (dev machine only; the generated file is committed):
    python3 scripts/refresh_vendor_catalog.py [--input api.json] [--check]

Downloads https://models.dev/api.json (or reads --input), keeps the
whitelisted providers verbatim, sorts keys for reproducible diffs, and writes
vendor/catalog.json with a `_meta` block. `--check` verifies the existing
snapshot against the whitelist (size budget, provider presence) without
writing — used by CI-style anchor guards.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import sys
import urllib.request
from pathlib import Path

API_URL = "https://models.dev/api.json"
SIZE_BUDGET_BYTES = 200 * 1024

# Whitelist by models.dev provider key. `ollama-cloud` is the Ollama Cloud
# entry (the plain `ollama` key does not exist). A missing key is an error,
# never a silent skip: a renamed provider must be a conscious decision.
WHITELIST = [
    "zhipuai",
    "zai",
    "zai-coding-plan",
    "xiaomi",
    "xiaomi-token-plan-cn",
    "alibaba",
    "alibaba-coding-plan-cn",
    "stepfun",
    "stepfun-step-plan",
    "deepseek",
    "minimax",
    "minimax-cn-coding-plan",
    "moonshotai",
    "siliconflow",
    "ollama-cloud",
]

# Anchors the vendor_catalog tests rely on: provider -> model -> context
# window. A drift that breaks these is meant to be loud.
ANCHORS = {
    "zai-coding-plan": {"glm-5.3-flash": 1_000_000},
}


def load_raw(input_path: str | None) -> dict:
    if input_path:
        return json.loads(Path(input_path).read_text())
    with urllib.request.urlopen(API_URL, timeout=60) as response:
        return json.loads(response.read())


def build(raw: dict) -> dict:
    missing = [key for key in WHITELIST if key not in raw]
    if missing:
        raise SystemExit(f"whitelist keys missing upstream: {missing}")
    providers = {
        key: raw[key]
        for key in WHITELIST
        if key in raw
    }
    return {
        "_meta": {
            "source": API_URL,
            "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(timespec="seconds"),
            "providers": sorted(providers.keys()),
            "size_budget_bytes": SIZE_BUDGET_BYTES,
        },
        "providers": providers,
    }


def verify(snapshot: dict) -> None:
    keys = sorted(snapshot.get("providers", {}).keys())
    missing = [key for key in WHITELIST if key not in keys]
    if missing:
        raise SystemExit(f"snapshot is missing whitelisted providers: {missing}")
    for provider, models in ANCHORS.items():
        entries = snapshot["providers"].get(provider, {}).get("models", {})
        for model, context in models.items():
            entry = entries.get(model)
            if entry is None:
                raise SystemExit(f"anchor missing: {provider}/{model}")
            actual = (entry.get("limit") or {}).get("context")
            if actual != context:
                raise SystemExit(
                    f"anchor drifted: {provider}/{model} limit.context "
                    f"{actual} != {context}"
                )
    size = len(json.dumps(snapshot, sort_keys=True).encode())
    if size > SIZE_BUDGET_BYTES:
        raise SystemExit(f"snapshot {size} bytes exceeds budget {SIZE_BUDGET_BYTES}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", help="path to a downloaded api.json")
    parser.add_argument("--check", action="store_true", help="verify, do not write")
    args = parser.parse_args()

    snapshot = build(load_raw(args.input))
    verify(snapshot)
    if args.check:
        print("catalog check passed")
        return

    out_path = Path(__file__).resolve().parent.parent / "vendor" / "catalog.json"
    out_path.parent.mkdir(parents=True, exist_ok=True)
    # Compact + sorted keys: deterministic diffs and the shipped bytes are
    # exactly what the size budget measures.
    out_path.write_text(json.dumps(snapshot, sort_keys=True) + "\n")
    size = out_path.stat().st_size
    if size > SIZE_BUDGET_BYTES:
        raise SystemExit(f"written snapshot {size} bytes exceeds budget {SIZE_BUDGET_BYTES}")
    print(f"wrote {out_path} ({size} bytes, budget {SIZE_BUDGET_BYTES})")


if __name__ == "__main__":
    main()
