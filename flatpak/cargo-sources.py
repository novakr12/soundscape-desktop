import json
import sys
import tomllib
from pathlib import Path

CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"


def sources(lock: dict) -> list[dict]:
    result = []
    for package in lock.get("package", []):
        source = package.get("source")
        if source is None:
            continue
        if source != CRATES_IO:
            raise SystemExit(f"{package['name']} comes from {source}; only crates.io packages are supported")
        name, version, checksum = package["name"], package["version"], package["checksum"]
        dest = f"cargo/vendor/{name}-{version}"
        result.append(
            {
                "type": "archive",
                "archive-type": "tar-gzip",
                "url": f"https://static.crates.io/crates/{name}/{name}-{version}.crate",
                "sha256": checksum,
                "dest": dest,
            }
        )
        result.append(
            {
                "type": "inline",
                "contents": json.dumps({"package": checksum, "files": {}}),
                "dest": dest,
                "dest-filename": ".cargo-checksum.json",
            }
        )
    result.append(
        {
            "type": "inline",
            "contents": '[source.vendored-sources]\ndirectory = "cargo/vendor"\n\n[source.crates-io]\nreplace-with = "vendored-sources"\n',
            "dest": "cargo",
            "dest-filename": "config.toml",
        }
    )
    return result


def main() -> None:
    here = Path(__file__).resolve().parent
    lock_path = Path(sys.argv[1]) if len(sys.argv) > 1 else here.parent / "Cargo.lock"
    out_path = Path(sys.argv[2]) if len(sys.argv) > 2 else here / "cargo-sources.json"
    with open(lock_path, "rb") as file:
        lock = tomllib.load(file)
    result = sources(lock)
    out_path.write_text(json.dumps(result, indent=2) + "\n")
    print(f"Wrote {(len(result) - 1) // 2} crates to {out_path}")


if __name__ == "__main__":
    main()
