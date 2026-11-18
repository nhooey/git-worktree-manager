"""What the shell learns about a repository, read-only: the keep patterns `clean` honours."""

from __future__ import annotations

import os
from typing import Mapping, Tuple

from . import core, system

# Keep patterns and kept files ---------------------------------------------------


def keep_patterns(cfg: Mapping, worktree: str, main: str) -> Tuple[str, ...]:
    """The defaults' keep patterns, then the repository's `.worktreeinclude` and `.worktree-keep`."""

    def from_file(name: str) -> Tuple[str, ...]:
        here = system.read_text(os.path.join(worktree, name))
        return core.parse_pattern_file(
            (here if here is not None else system.read_text(os.path.join(main, name))) or ""
        )

    return tuple(cfg["keep"]) + from_file(".worktreeinclude") + from_file(".worktree-keep")
