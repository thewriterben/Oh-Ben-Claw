"""Shared Rust-import parsing for the survey scripts.

Lifted out of `curation_survey.py` on 2026-08-01, when `extractability.py`
needed the same brace matcher. Two copies of this parser would have been the
exact failure those scripts exist to report: the 2026-07-29 correction to the
use-tree matcher took two wrong answers with it, and a second copy would have
kept one of them alive somewhere.

Nothing here knows what a module *means* — that judgement belongs to the
callers, which ask opposite questions of the same data. `curation_survey` asks
who references a module (can it be deleted); `extractability` asks what a module
references (can it be moved).
"""

from __future__ import annotations

import re
from pathlib import Path

CRATE_ROOT = re.compile(r"\buse\s+(?:crate|oh_ben_claw)\s*::\s*")


def declared_modules(lib_rs: Path) -> tuple[list[str], list[str]]:
    """`(pub mod names, pub use aliases)` from `src/lib.rs`.

    The crate's own view of what a module is. Directory listing would also pick
    up `src/bin/` (a Cargo convention, not a module) and any scratch directory.

    The second list is the extracted crates: `pub use obc_memory as memory;`
    means `crate::memory` resolves outside this crate entirely, so a reference to
    it does not tie a module to the tree.
    """
    text = lib_rs.read_text(encoding="utf-8")
    mods = re.findall(r"^\s*pub\s+mod\s+([a-z_][a-z0-9_]*)\s*;", text, re.M)
    aliases = re.findall(
        r"^\s*pub\s+use\s+([a-z_][a-z0-9_:]*?)(?:\s+as\s+([a-z_][a-z0-9_]*))?\s*;",
        text,
        re.M,
    )
    external = []
    for path, alias in aliases:
        name = alias or path.split("::")[-1]
        # `pub use config::Config;` re-exports from inside this crate; only paths
        # rooted at another crate (obc_*) leave it.
        if path.startswith("obc_"):
            external.append(name)
    return sorted(mods), sorted(set(external))


def use_tree_heads(text: str) -> set[str]:
    """Top-level module names imported by any `use crate::{…}` / `use oh_ben_claw::{…}`.

    Brace-matched rather than regexed, so nested groups and multi-line imports
    both work. `use crate::{a, b::{c, d}, e as f}` yields {a, b, e}.
    """
    heads: set[str] = set()
    for m in CRATE_ROOT.finditer(text):
        i = m.end()
        if i >= len(text):
            continue
        if text[i] != "{":
            # `use crate::foo::…;` — single path, head is the next identifier.
            ident = re.match(r"([a-z_][a-z0-9_]*)", text[i:])
            if ident:
                heads.add(ident.group(1))
            continue
        depth, j = 0, i
        while j < len(text):
            if text[j] == "{":
                depth += 1
            elif text[j] == "}":
                depth -= 1
                if depth == 0:
                    break
            j += 1
        group = text[i + 1:j]
        # split on top-level commas only
        depth, start = 0, 0
        parts = []
        for k, ch in enumerate(group):
            if ch == "{":
                depth += 1
            elif ch == "}":
                depth -= 1
            elif ch == "," and depth == 0:
                parts.append(group[start:k])
                start = k + 1
        parts.append(group[start:])
        for part in parts:
            ident = re.match(r"\s*([a-z_][a-z0-9_]*)", part)
            if ident:
                heads.add(ident.group(1))
    return heads


def direct_ref(module: str) -> re.Pattern[str]:
    """Matches an inline `crate::module::…` path, which no `use` line records."""
    return re.compile(rf"\b(?:crate|oh_ben_claw)\s*::\s*{re.escape(module)}\b")
