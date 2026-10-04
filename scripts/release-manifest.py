#!/usr/bin/env python3
"""Add a release to the update channel manifest (stable.json).

Signs each tarball with the release key; prereleases never enter the
stable channel. The manifest itself is not signed: the per-artifact
signature is what an install checks before running anything.
"""
import argparse
import hashlib
import json
import pathlib
import re
import sys

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

ROOT = pathlib.Path(__file__).resolve().parent.parent
# vyasa-<version>-<target>.tar.gz, as scripts/package-release.sh names it.
TARBALL = re.compile(r"^vyasa-(?P<v>\d+\.\d+\.\d+)-(?P<target>[a-z0-9_]+-[a-z0-9_]+-[a-z0-9_]+(?:-[a-z0-9_]+)?)\.tar\.gz$")


def main(argv=None) -> int:
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--version", required=True)
    p.add_argument("--dist", required=True, type=pathlib.Path)
    p.add_argument("--key-file", required=True, type=pathlib.Path)
    p.add_argument("--current", type=pathlib.Path, help="the manifest published so far")
    p.add_argument("--out", required=True, type=pathlib.Path)
    p.add_argument("--summary", default="")
    a = p.parse_args(argv)
    if "-" in a.version:
        print(f"{a.version} is a prerelease; the stable channel takes releases only", file=sys.stderr)
        return 2
    key = Ed25519PrivateKey.from_private_bytes(bytes.fromhex(a.key_file.read_text().strip()))
    artifacts = []
    for f in sorted(a.dist.glob(f"vyasa-{a.version}-*.tar.gz")):
        m = TARBALL.match(f.name)
        if not m:
            continue
        data = f.read_bytes()
        artifacts.append({
            "platform": m.group("target"),
            "url": f"https://github.com/vyasa-cms/vyasa/releases/download/v{a.version}/{f.name}",
            "sha256": hashlib.sha256(data).hexdigest(),
            "signature": key.sign(data).hex(),
        })
    if not artifacts:
        print(f"no vyasa-{a.version}-*.tar.gz in {a.dist}", file=sys.stderr)
        return 1
    doc = {"latest": "0.0.0", "releases": []}
    if a.current and a.current.exists():
        doc = json.loads(a.current.read_text())
    doc["releases"] = [r for r in doc.get("releases", []) if r.get("version") != a.version]
    doc["releases"].append({
        "version": a.version,
        "notes_url": f"https://github.com/vyasa-cms/vyasa/releases/tag/v{a.version}",
        "summary": a.summary,
        "migration_version": len(list((ROOT / "crates/db/migrations").glob("*.sql"))),
        "artifacts": artifacts,
    })
    doc["releases"].sort(key=lambda r: tuple(int(x) for x in r["version"].split(".")), reverse=True)
    doc["latest"] = doc["releases"][0]["version"]
    a.out.write_text(json.dumps(doc, indent=2) + "\n")
    print(f"stable.json: latest {doc['latest']}, {len(doc['releases'])} release(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
