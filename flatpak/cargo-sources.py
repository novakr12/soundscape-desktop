import hashlib
import io
import json
import sys
import tarfile
import tomllib
import urllib.request
from pathlib import Path

CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"
GLUE_PREFIX = "mozjs_"


def crate_url(name: str, version: str) -> str:
    return f"https://static.crates.io/crates/{name}/{name}-{version}.crate"


def bundled_lock(package: dict) -> dict | None:
    name, version = package["name"], package["version"]
    with urllib.request.urlopen(crate_url(name, version)) as response:
        data = response.read()
    if hashlib.sha256(data).hexdigest() != package["checksum"]:
        raise SystemExit(f"{name} {version} doesn't match the checksum in Cargo.lock")
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        names = set(archive.getnames())
        build = f"{name}-{version}/build.rs"
        lock = f"{name}-{version}/Cargo.lock"
        if build not in names or lock not in names:
            return None
        if b"cbindgen" not in archive.extractfile(build).read():
            return None
        return tomllib.loads(archive.extractfile(lock).read().decode())


def with_bundled_locks(lock: dict) -> dict:
    packages = list(lock.get("package", []))
    known = {(package["name"], package["version"]) for package in packages}
    for package in lock.get("package", []):
        if not package["name"].startswith(GLUE_PREFIX) or package.get("source") != CRATES_IO:
            continue
        extra = bundled_lock(package)
        if extra is None:
            continue
        for found in extra.get("package", []):
            key = (found["name"], found["version"])
            if found.get("source") == CRATES_IO and key not in known:
                known.add(key)
                packages.append(found)
    return {**lock, "package": packages}


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
                "url": crate_url(name, version),
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
    result = sources(with_bundled_locks(lock))
    out_path.write_text(json.dumps(result, indent=2) + "\n")
    print(f"Wrote {(len(result) - 1) // 2} crates to {out_path}")


if __name__ == "__main__":
    main()
