import re
import subprocess
import sys
from pathlib import Path

REPOSITORY = "https://github.com/novakr12/soundscape-desktop.git"
SOURCE = re.compile(r"      - type: dir\n        path: \.\.\n        skip:\n(?:          - .*\n)+")


def main() -> None:
    if len(sys.argv) != 2 or not sys.argv[1].startswith("v"):
        raise SystemExit("Usage: python3 flatpak/flathub-manifest.py v0.1.0")
    tag = sys.argv[1]
    here = Path(__file__).resolve().parent
    commit = subprocess.run(
        ["git", "rev-list", "-n", "1", tag], cwd=here, check=True, capture_output=True, text=True
    ).stdout.strip()
    name = "io.github.novakr12.SoundscapeDesktop.yml"
    manifest = (here / name).read_text()
    replaced, count = SOURCE.subn(
        f"      - type: git\n        url: {REPOSITORY}\n        tag: {tag}\n        commit: {commit}\n", manifest
    )
    if count != 1:
        raise SystemExit("Couldn't find the local source in the manifest")
    out = here / "flathub"
    out.mkdir(exist_ok=True)
    (out / name).write_text(replaced)
    print(f"Wrote {out / name} for {tag} ({commit[:12]})")


if __name__ == "__main__":
    main()
