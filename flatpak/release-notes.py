import json
import sys
import tomllib
import xml.etree.ElementTree as ElementTree
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
METAINFO = ROOT / "flatpak" / "io.github.novakr12.SoundscapeDesktop.metainfo.xml"
CARGO = ROOT / "src-tauri" / "Cargo.toml"
TAURI = ROOT / "src-tauri" / "tauri.conf.json"


def text(element):
    return " ".join("".join(element.itertext()).split())


def markdown(description):
    lines = []
    for element in description:
        if element.tag == "p":
            lines += [text(element), ""]
        elif element.tag in ("ul", "ol"):
            lines += [f"- {text(item)}" for item in element.findall("li")] + [""]
    return lines


def main():
    if len(sys.argv) != 2:
        sys.exit("Usage: release-notes.py <tag, like v0.2.0>")
    tag = sys.argv[1]
    version = tag.removeprefix("v")
    cargo = tomllib.loads(CARGO.read_text())["package"]["version"]
    tauri = json.loads(TAURI.read_text())["version"]
    releases = ElementTree.parse(METAINFO).getroot().findall("./releases/release")
    newest = releases[0].get("version") if releases else None
    problems = [
        f"{name} says {found}"
        for name, found in (("Cargo.toml", cargo), ("tauri.conf.json", tauri), ("the newest metainfo release", newest))
        if found != version
    ]
    if problems:
        sys.exit(f"Tag {tag} doesn't match: " + ", ".join(problems))
    release = releases[0]
    description = release.find("description")
    lines = markdown(description) if description is not None else []
    lines += [
        "## Install",
        "",
        f"Download `soundscape-desktop-{version}-x86_64.flatpak` below and run:",
        "",
        "```",
        f"flatpak install --user soundscape-desktop-{version}-x86_64.flatpak",
        "```",
        "",
        "You need a Soundscape server new enough to have desktop app support.",
    ]
    print("\n".join(lines))


main()
