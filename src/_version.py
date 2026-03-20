"""Application version and GitHub release-check helpers."""
from __future__ import annotations

__version__ = "1.0.9"
GITHUB_REPO  = "nyakuoff/Rivals-Swapper"


def fetch_latest_release(repo: str) -> tuple[str, str] | None:
    """Return ``(tag_name, html_url)`` of the latest GitHub release, or None."""
    import json
    import urllib.request

    url = f"https://api.github.com/repos/{repo}/releases/latest"
    try:
        req = urllib.request.Request(url, headers={"User-Agent": "RivalsSwapper"})
        with urllib.request.urlopen(req, timeout=6) as resp:
            data = json.loads(resp.read())
        return data.get("tag_name", ""), data.get("html_url", "")
    except Exception:
        return None


def parse_version(v: str) -> tuple[int, ...]:
    """Parse ``v1.2.3`` or ``1.2.3`` into a comparable int tuple."""
    v = v.lstrip("v").strip()
    try:
        return tuple(int(x) for x in v.split("."))
    except Exception:
        return (0,)
