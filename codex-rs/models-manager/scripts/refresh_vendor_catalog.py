#!/usr/bin/env python3
"""Refresh the bundled vendor model catalog.

Two inputs, in this order of authority:

1. **rig's catalog** (`rig-core-<ver>/src/catalog/models.json`, compiled into
   the rig we depend on). This is the freshest models.dev snapshot we already
   ship, and the fork's rules track it: upgrading rig upgrades the facts.
   Rows are matched by `canonical_model_id` (falling back to the model id) and
   merged *over* the existing entry, so provider keys, plan variants and the
   whitelist stay exactly as the fork defines them.
2. **models.dev `api.json`** (`--input`, optional). Only needed to (re)build
   entries rig does not carry — Qwen/Alibaba, StepFun, SiliconFlow, Zhipu
   official and the plan-style provider keys. Without it the script refreshes
   the committed snapshot in place.

Usage (dev machine only; the generated file is committed):

    python3 scripts/refresh_vendor_catalog.py                # refresh from rig
    python3 scripts/refresh_vendor_catalog.py --check        # verify only
    python3 scripts/refresh_vendor_catalog.py --input api.json   # extend/rebuild

`--check` verifies the committed snapshot against the whitelist, the anchors
and the size budget without writing — used by CI-style guards.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import sys
import urllib.request
from pathlib import Path

API_URL = "https://models.dev/api.json"
SIZE_BUDGET_BYTES = 224 * 1024

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

# Our provider key -> rig's provider key. rig normalizes to one key per
# vendor, so the fork's plan-style keys all point at the vendor's base key.
# Providers absent here (alibaba*, stepfun*, siliconflow, ollama-cloud) have
# no rig counterpart and are only ever refreshed from models.dev.
RIG_PROVIDER_MAP = {
    "zhipuai": "zai",
    "zai": "zai",
    "zai-coding-plan": "zai",
    "xiaomi": "xiaomimimo",
    "xiaomi-token-plan-cn": "xiaomimimo",
    "deepseek": "deepseek",
    "minimax": "minimax",
    "minimax-cn-coding-plan": "minimax",
    "moonshotai": "moonshot",
}

# Anchors the vendor_catalog tests rely on: provider -> model -> context
# window. A drift that breaks these is meant to be loud.
ANCHORS = {
    "zai-coding-plan": {"glm-5.3-flash": 1_000_000},
    # The output cap is a live dependency of the request builder (see
    # `vendor_catalog::output_limit_for`): a 1M-window gateway taking 128k
    # must not be sent `window/2`.
    "xiaomi-token-plan-cn": {"mimo-v2.6-flash": 1_048_576},
}

# The fields rig's snapshot records that the fork reads (or will read).
# Everything else rides along verbatim — the snapshot is a data file, not a
# schema.
MERGED_FIELDS = (
    "attachment",
    "canonical_model_id",
    "cost",
    "description",
    "family",
    "interleaved",
    "knowledge",
    "last_updated",
    "limit",
    "modalities",
    "name",
    "open_weights",
    "reasoning",
    "reasoning_options",
    "release_date",
    "status",
    "structured_output",
    "temperature",
    "tool_call",
)


def repo_root() -> Path:
    return Path(__file__).resolve().parent.parent


def default_rig_catalog() -> Path:
    """The models.json of the rig-core the workspace actually resolves."""
    override = os.environ.get("RIG_CATALOG")
    if override:
        return Path(override)
    cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    candidates = sorted(
        (cargo_home / "registry" / "src").glob("*/rig-core-*/src/catalog/models.json")
    )
    if not candidates:
        raise SystemExit(
            "rig catalog not found; pass --rig <path to rig-core .../catalog/models.json>"
        )
    return candidates[-1]


def load_rig(path: Path) -> tuple[dict, str | None]:
    raw = json.loads(path.read_text())
    # rig's own snapshot date sits next to the file; keep it for provenance.
    generated_at = None
    stamp = path.parent / "generated_at"
    if stamp.is_file():
        generated_at = stamp.read_text().strip() or None
    return raw, generated_at


def load_models_dev(input_path: str | None) -> dict:
    if input_path:
        return json.loads(Path(input_path).read_text())
    with urllib.request.urlopen(API_URL, timeout=60) as response:
        return json.loads(response.read())


def rig_index(rig: dict, provider: str) -> tuple[dict, dict]:
    """(canonical id -> row, model id -> row) for one rig provider."""
    rows = rig.get(provider, {}).get("models", {})
    by_canonical: dict[str, dict] = {}
    by_id: dict[str, dict] = {}
    for model_id, row in rows.items():
        by_id[model_id.lower()] = row
        canonical = row.get("canonical_model_id")
        if isinstance(canonical, str) and canonical:
            by_canonical[canonical.lower()] = row
    return by_canonical, by_id


def merged_entry(entry: dict, model_id: str, rig_row: dict | None) -> dict:
    merged = dict(entry)
    if rig_row:
        for field in MERGED_FIELDS:
            if field in rig_row:
                merged[field] = rig_row[field]
    merged.setdefault("id", model_id)
    return merged


def build(
    rig: dict,
    rig_generated_at: str | None,
    rig_path: Path,
    models_dev: dict | None,
) -> dict:
    if models_dev is not None:
        missing = [key for key in WHITELIST if key not in models_dev]
        if missing:
            raise SystemExit(f"whitelist keys missing upstream: {missing}")
        providers = {key: json.loads(json.dumps(models_dev[key])) for key in WHITELIST}
    else:
        committed = repo_root() / "vendor" / "catalog.json"
        if not committed.is_file():
            raise SystemExit(
                f"{committed} does not exist; pass --input <models.dev api.json> to build it"
            )
        providers = json.loads(committed.read_text())["providers"]

    refreshed = 0
    added = 0
    for provider, payload in providers.items():
        rig_provider = RIG_PROVIDER_MAP.get(provider)
        if rig_provider is None:
            continue
        by_canonical, by_id = rig_index(rig, rig_provider)
        if not by_canonical and not by_id:
            continue
        models = payload.setdefault("models", {})
        for model_id in list(models):
            entry = models[model_id]
            canonical = (entry.get("canonical_model_id") or "").lower()
            rig_row = by_canonical.get(canonical) if canonical else None
            rig_row = rig_row or by_id.get(model_id.lower())
            if rig_row is None:
                continue
            models[model_id] = merged_entry(entry, model_id, rig_row)
            refreshed += 1
        # Models rig knows that this provider key does not carry yet.
        known = {
            (entry.get("canonical_model_id") or "").lower() for entry in models.values()
        } | {model_id.lower() for model_id in models}
        for model_id, rig_row in sorted(by_id.items()):
            canonical = (rig_row.get("canonical_model_id") or "").lower()
            if model_id in known or (canonical and canonical in known):
                continue
            # Only text models with a declared window: image/speech rows and
            # previews without `limit.context` would land in the catalog with
            # no window at all, which budgets worse than the placeholder.
            if not (rig_row.get("limit") or {}).get("context"):
                continue
            models[model_id] = merged_entry({}, model_id, rig_row)
            added += 1

    return {
        "_meta": {
            "source": "rig-core catalog/models.json",
            "rig_generated_at": rig_generated_at,
            "rig_path": str(rig_path),
            "models_dev": API_URL if models_dev is not None else None,
            "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(timespec="seconds"),
            "providers": sorted(providers.keys()),
            "refreshed_entries": refreshed,
            "added_entries": added,
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
    for provider, models in ANCHORS.items():
        entries = snapshot["providers"].get(provider, {}).get("models", {})
        for model in models:
            output = (entries[model].get("limit") or {}).get("output")
            if output is None:
                raise SystemExit(f"anchor lost its output limit: {provider}/{model}")
    size = len(json.dumps(snapshot, sort_keys=True).encode())
    if size > SIZE_BUDGET_BYTES:
        raise SystemExit(f"snapshot {size} bytes exceeds budget {SIZE_BUDGET_BYTES}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--rig",
        help="path to rig-core's catalog/models.json (default: the resolved rig-core)",
    )
    parser.add_argument(
        "--input",
        help="path to a downloaded models.dev api.json; rebuilds entries rig cannot refresh",
    )
    parser.add_argument("--check", action="store_true", help="verify, do not write")
    args = parser.parse_args()

    if args.check:
        committed = json.loads((repo_root() / "vendor" / "catalog.json").read_text())
        verify(committed)
        print(
            "catalog check passed "
            f"({sum(len(p.get('models', {})) for p in committed['providers'].values())} entries)"
        )
        return

    rig_path = Path(args.rig) if args.rig else default_rig_catalog()
    rig, rig_generated_at = load_rig(rig_path)
    models_dev = load_models_dev(args.input) if args.input else None
    snapshot = build(rig, rig_generated_at, rig_path, models_dev)
    verify(snapshot)

    out_path = repo_root() / "vendor" / "catalog.json"
    out_path.parent.mkdir(parents=True, exist_ok=True)
    # Compact + sorted keys: deterministic diffs and the shipped bytes are
    # exactly what the size budget measures.
    out_path.write_text(json.dumps(snapshot, sort_keys=True) + "\n")
    size = out_path.stat().st_size
    if size > SIZE_BUDGET_BYTES:
        raise SystemExit(f"written snapshot {size} bytes exceeds budget {SIZE_BUDGET_BYTES}")
    meta = snapshot["_meta"]
    print(
        f"wrote {out_path} ({size} bytes, budget {SIZE_BUDGET_BYTES}; "
        f"refreshed {meta['refreshed_entries']}, added {meta['added_entries']}, "
        f"rig snapshot {rig_generated_at or '<unknown>'})"
    )


if __name__ == "__main__":
    sys.exit(main())
