#!/usr/bin/env python3
"""Check new Cargo.lock entries against crates.io without executing crate code.

Requires Python 3.11+. Run from any directory; --base must exist in local Git
history. Registry metadata errors fail closed rather than bypassing quarantine.
"""

import argparse
from datetime import datetime, timedelta, timezone
import json
from pathlib import Path
import subprocess
import sys
import time
import tomllib
from urllib.error import HTTPError, URLError
from urllib.parse import quote
from urllib.request import Request, urlopen

ROOT = Path(__file__).resolve().parents[2]
CRATES_IO = "registry+https://github.com/rust-lang/crates.io-index"
COOLDOWN = timedelta(days=1)


def changed_packages(before: str, after: str) -> list[dict]:
    """Include all new external entries, including transitive/target-only crates."""

    def entries(text):
        return {
            (p["name"], p["version"], p["source"], p.get("checksum")): p
            for p in tomllib.loads(text)["package"]
            if "source" in p
        }

    old, new = entries(before), entries(after)
    return [new[key] for key in sorted(new.keys() - old.keys())]


def fetch_version(name: str, version: str) -> dict:
    """Fetch metadata over HTTPS with bounded retries and no third-party modules."""
    url = f"https://crates.io/api/v1/crates/{quote(name, safe='')}/{quote(version, safe='')}"
    request = Request(
        url,
        headers={
            "User-Agent": "dbcrust-dependency-check (https://github.com/clement-tourriere/dbcrust)",
        },
    )
    for attempt in range(3):
        # Stay below one request per second, including retries.
        time.sleep(2**attempt)
        try:
            with urlopen(request, timeout=20) as response:
                return json.load(response)["version"]
        except HTTPError as error:
            if attempt == 2 or (error.code != 429 and error.code < 500):
                raise
        except (URLError, OSError):
            if attempt == 2:
                raise
    raise RuntimeError("crates.io metadata unavailable")


def verify_version(package: dict, metadata: dict, now: datetime) -> None:
    """Reject wrong metadata, checksum mismatches, yanks, and releases under 24h."""
    if (metadata["crate"], metadata["num"]) != (package["name"], package["version"]):
        raise ValueError("crates.io returned metadata for a different release")
    if metadata["checksum"] != package["checksum"]:
        raise ValueError("Cargo.lock checksum does not match crates.io")
    if metadata["yanked"] is not False:
        raise ValueError("release is yanked or its yank status is unknown")
    published = datetime.fromisoformat(metadata["created_at"].replace("Z", "+00:00"))
    if published.tzinfo is None:
        raise ValueError("publication timestamp has no timezone")
    eligible = published + COOLDOWN
    if now < eligible:
        raise ValueError(
            f"release is under 24 hours old; retry after {eligible.isoformat()}"
        )


def check_packages(packages: list[dict], now: datetime) -> list[str]:
    """Return failures; unknown registries/Git sources require policy review."""
    failures = []
    for package in packages:
        try:
            if package["source"] != CRATES_IO:
                raise ValueError(
                    "new non-crates.io source requires an explicit policy review"
                )
            metadata = fetch_version(package["name"], package["version"])
            verify_version(package, metadata, now)
        except (
            OSError,
            URLError,
            ValueError,
            KeyError,
            TypeError,
            AttributeError,
        ) as error:
            failures.append(f"{package['name']}@{package['version']}: {error}")
    return failures


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", default="HEAD", help="Git base ref (default: HEAD)")
    args = parser.parse_args()
    try:
        if not args.base:
            raise ValueError("Git base ref must not be empty")
        # GitHub uses an all-zero SHA for a newly created branch.
        if args.base == "0" * 40:
            before = "package = []"
        else:
            before = subprocess.check_output(
                ["git", "show", "--end-of-options", f"{args.base}:Cargo.lock"],
                cwd=ROOT,
                text=True,
            )
        packages = changed_packages(before, (ROOT / "Cargo.lock").read_text())
        print(f"Checking {len(packages)} new external Cargo.lock entries", flush=True)
        failures = check_packages(packages, datetime.now(timezone.utc))
    except (
        OSError,
        subprocess.CalledProcessError,
        ValueError,
        KeyError,
        TypeError,
    ) as error:
        print(f"Cooldown check could not complete: {error}", file=sys.stderr)
        return 1
    for failure in failures:
        print(f"ERROR: {failure}", file=sys.stderr)
    if failures:
        return 1
    print("Cargo cooldown/checksum/yank checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
