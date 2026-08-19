"""One line of console setup, in one place, because it has been written twice.

Every survey in here draws its sections with box characters. A Windows console
defaults to cp1252, which cannot encode them, so `print("── x ──")` raises
UnicodeEncodeError — *after* the survey has done all its work and often after it
has printed its counts. It reads as a broken tool and is a broken terminal.

`file_reachability.py` hit it on 2026-08-13 and got a fix. `inert_components.py`
hit it on 2026-08-14 and got a copy of the same fix, with a note in the commit
saying the guard belongs to *printing a rule* rather than to that one survey,
and that it should be copied into the next script before it crashed rather than
after.

`curation_survey.py` hit it the next day, on its first run, because the note was
advice and advice is not a mechanism. So it is a function now. A third copy
would have been this repository's own recurring finding — an instrument that
records a lesson it does not enforce — performed on itself.
"""

from __future__ import annotations

import sys


def use_utf8_stdout() -> None:
    """Make stdout and stderr able to carry the box characters we print.

    A no-op where they already are, which is everywhere this runs unattended.
    Best-effort: a stream without `reconfigure` (a pipe wrapper, a test double)
    is left alone rather than raising, because failing to set an encoding must
    not be the thing that stops a survey from running.
    """
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(encoding="utf-8", errors="replace")
