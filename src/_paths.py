"""
Centralised path resolution — works both from source and from a
PyInstaller bundle.

When frozen (PyInstaller):
    • BUNDLE_DIR   = sys._MEIPASS  (_internal/)  — bundled data (assets/, tools/)
    • PROJECT_ROOT = exe's parent directory       — user data (data/, config/, output/)

When running from source:
    • BUNDLE_DIR   = <repo>/                      — same as PROJECT_ROOT
    • PROJECT_ROOT = <repo>/
"""

import sys
from pathlib import Path

_frozen = getattr(sys, "frozen", False)

if _frozen:
    # PyInstaller sets sys._MEIPASS to the _internal/ temp dir
    BUNDLE_DIR = Path(sys._MEIPASS)
    # The exe sits one level above _internal/
    PROJECT_ROOT = Path(sys.executable).resolve().parent
else:
    # Running from source: src/_paths.py → parent = src/ → parent = repo root
    PROJECT_ROOT = Path(__file__).resolve().parent.parent
    BUNDLE_DIR = PROJECT_ROOT

ASSETS_DIR = BUNDLE_DIR / "assets"
TOOLS_DIR  = BUNDLE_DIR / "tools"
