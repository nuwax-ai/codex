#!/usr/bin/env python3
"""v2: extract 'codex' string literals from production Rust code.

- Excludes files whose names match /test/i (separate test files).
- Excludes files pulled in via #[cfg(test)] #[path="..."] modules.
- Skips #[cfg(test)] mod blocks and #[test]/#[cfg(test)] fns by indent-based
  end detection (repo is rustfmt'd, closing brace sits at the decl's indent).
- Strips // comments (but not :// inside strings).
- Scans normal "..." literals plus r#"..."# raw strings spanning kept lines.
Prints: file:line:"literal"
"""
import re
import sys
from pathlib import Path

TEST_FILE = re.compile(r"test", re.I)
LITERAL_RE = re.compile(r'"((?:[^"\\]|\\.)*)"')
RAW_RE = re.compile(r'r#"(.*?)"#', re.S)
COMMENT_RE = re.compile(r"(?<!:)//.*$")
ATTR_PATH = re.compile(r'#\[path\s*=\s*"([^"]+)"\]')
CFG_TEST = re.compile(r"#\[\s*cfg\s*\(\s*test\s*\)\s*\]")
TEST_FN_ATTR = re.compile(r"#\[\s*(test|tokio::test)\b")
DECL_RE = re.compile(
    r"(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:unsafe\s+)?(mod|fn)\s+(\w+)"
)


def kept_lines(lines):
    """Return list of (lineno, code) outside test mods/fns, comments stripped."""
    kept = []
    i, n = 0, len(lines)
    while i < n:
        line = lines[i]
        stripped = line.strip()
        indent = len(line) - len(line.lstrip())
        skip_block = None
        if CFG_TEST.match(stripped) or TEST_FN_ATTR.match(stripped):
            # gather attributes then the decl
            j = i
            while j < n and lines[j].strip().startswith("#["):
                j += 1
            if j < n:
                m = DECL_RE.match(lines[j].strip())
                if m:
                    decl_line = lines[j]
                    decl_indent = len(decl_line) - len(decl_line.lstrip())
                    # external mod decl ends with ';' -> nothing to skip inline
                    if not decl_line.rstrip().endswith(";"):
                        skip_block = decl_indent
                        i = j  # continue skipping from decl line
        if skip_block is not None:
            i += 1
            while i < n:
                s = lines[i]
                if s.strip() == "}" and (len(s) - len(s.lstrip())) == skip_block:
                    break
                i += 1
            i += 1
            continue
        if stripped.startswith("//"):
            i += 1
            continue
        code = COMMENT_RE.sub("", line).rstrip("\n")
        if code.strip():
            kept.append((i + 1, code))
        i += 1
    return kept


def main():
    roots = sys.argv[1:]
    test_only_files = set()  # files referenced from cfg(test) path modules
    files = []
    for root in roots:
        for path in sorted(Path(root).rglob("*.rs")):
            if TEST_FILE.search(path.name) or any(
                TEST_FILE.search(part) for part in path.parts[:-1]
            ):
                continue
            files.append(path)
    for path in files:
        text = path.read_text(errors="replace")
        lines = text.splitlines()
        # collect cfg(test) #[path] module targets
        for idx, line in enumerate(lines):
            if CFG_TEST.search(line):
                window = "\n".join(lines[idx : idx + 4])
                m = ATTR_PATH.search(window)
                if m and "mod" in window:
                    test_only_files.add((path.parent / m.group(1)).resolve())
    for path in files:
        if path.resolve() in test_only_files:
            continue
        lines = path.read_text(errors="replace").splitlines()
        kept = kept_lines(lines)
        for lineno, code in kept:
            for m in LITERAL_RE.finditer(code):
                if re.search(r"codex", m.group(1), re.I):
                    print(f'{path}:{lineno}:"{m.group(1)}"')
        blob = "\n".join(c for _, c in kept)
        for m in RAW_RE.finditer(blob):
            if re.search(r"codex", m.group(1), re.I):
                frag = m.group(1)
                head = frag.strip().splitlines()[0][:80]
                print(f'{path}:RAW#"{head}..."')


if __name__ == "__main__":
    main()
