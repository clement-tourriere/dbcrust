"""Offline tests for the Cargo supply-chain guard (stdlib unittest)."""

from datetime import datetime, timedelta, timezone
import io
import json
import subprocess
import unittest
from unittest.mock import patch
from urllib.error import HTTPError, URLError

import check_cargo_cooldown as guard

NOW = datetime(2026, 9, 8, 12, tzinfo=timezone.utc)
PACKAGE = {
    "name": "example-crate",
    "version": "1.2.3+build.1",
    "source": guard.CRATES_IO,
    "checksum": "a" * 64,
}


def metadata(age=timedelta(days=2)):
    return {
        "crate": PACKAGE["name"],
        "num": PACKAGE["version"],
        "checksum": PACKAGE["checksum"],
        "yanked": False,
        "created_at": (NOW - age).isoformat(),
    }


def lockfile(*packages):
    return (
        "package = []"
        if not packages
        else "\n".join(
            "[[package]]\n"
            + "\n".join(f"{key} = {json.dumps(value)}" for key, value in p.items())
            for p in packages
        )
    )


class CooldownTests(unittest.TestCase):
    def test_exact_boundary_and_future_release(self):
        for age, accepted in [
            (timedelta(days=2), True),
            (timedelta(days=1), True),
            (timedelta(days=1, microseconds=-1), False),
            (timedelta(0), False),
            (timedelta(hours=-1), False),
        ]:
            with self.subTest(age=age):
                if accepted:
                    guard.verify_version(PACKAGE, metadata(age), NOW)
                else:
                    with self.assertRaisesRegex(ValueError, "under 24 hours"):
                        guard.verify_version(PACKAGE, metadata(age), NOW)

    def test_timestamp_offsets_are_compared_in_utc(self):
        for timestamp in ["2026-09-07T12:00:00Z", "2026-09-07T14:00:00+02:00"]:
            with self.subTest(timestamp=timestamp):
                data = {**metadata(), "created_at": timestamp}
                guard.verify_version(PACKAGE, data, NOW)

    def test_bad_metadata_is_never_accepted(self):
        for field, value in [
            ("crate", "different-crate"),
            ("num", "2.0.0"),
            ("checksum", "b" * 64),
            ("yanked", True),
            ("yanked", None),
            ("yanked", "false"),
            ("created_at", "2026-09-01T00:00:00"),
            ("created_at", "not-a-timestamp"),
        ]:
            with self.subTest(field=field, value=value):
                with self.assertRaises(ValueError):
                    guard.verify_version(PACKAGE, {**metadata(), field: value}, NOW)
        with self.assertRaises(KeyError):
            guard.verify_version(PACKAGE, {}, NOW)

    def test_diff_covers_all_external_entries_not_only_direct_deps(self):
        old = {**PACKAGE, "version": "1.2.2"}
        transitive = {**PACKAGE, "name": "transitive-target-only"}
        workspace = {"name": "dbcrust", "version": "1.0.0"}
        unchanged = {**PACKAGE, "name": "unchanged"}
        actual = guard.changed_packages(
            lockfile(old, workspace, unchanged),
            lockfile(PACKAGE, transitive, workspace, unchanged),
        )
        self.assertEqual(actual, [PACKAGE, transitive])
        self.assertEqual(guard.changed_packages(lockfile(PACKAGE), lockfile()), [])
        self.assertEqual(
            guard.changed_packages(lockfile(PACKAGE), lockfile(PACKAGE)), []
        )

    def test_checksum_and_source_changes_are_rechecked(self):
        for changed in [
            {**PACKAGE, "checksum": "b" * 64},
            {**PACKAGE, "source": "git+https://example.org/repo#1234"},
        ]:
            with self.subTest(package=changed):
                self.assertEqual(
                    guard.changed_packages(lockfile(PACKAGE), lockfile(changed)),
                    [changed],
                )

    def test_unknown_source_is_rejected_without_network_access(self):
        with patch.object(guard, "fetch_version") as fetch:
            errors = guard.check_packages(
                [{**PACKAGE, "source": "registry+https://example.org"}], NOW
            )
            self.assertIn("policy review", errors[0])
            fetch.assert_not_called()

    def test_network_and_schema_errors_fail_closed(self):
        for error in [
            URLError("offline"),
            TimeoutError("timeout"),
            KeyError("version"),
        ]:
            with (
                self.subTest(error=error),
                patch.object(guard, "fetch_version", side_effect=error),
            ):
                self.assertEqual(len(guard.check_packages([PACKAGE], NOW)), 1)
        with patch.object(guard, "fetch_version", return_value=metadata()):
            self.assertEqual(guard.check_packages([PACKAGE], NOW), [])

    def test_fetch_retries_transient_failure_and_encodes_version(self):
        response = io.BytesIO(json.dumps({"version": metadata()}).encode())
        with (
            patch.object(guard.time, "sleep"),
            patch.object(
                guard, "urlopen", side_effect=[URLError("offline"), response]
            ) as fetch,
        ):
            self.assertEqual(
                guard.fetch_version(PACKAGE["name"], PACKAGE["version"]), metadata()
            )
            self.assertEqual(fetch.call_count, 2)
            self.assertIn("1.2.3%2Bbuild.1", fetch.call_args.args[0].full_url)

    def test_retry_limit_and_no_retry_for_404(self):
        for code, attempts in [(429, 3), (503, 3), (404, 1)]:
            error = HTTPError("https://crates.io/", code, "unavailable", {}, None)
            with (
                self.subTest(code=code),
                patch.object(guard.time, "sleep"),
                patch.object(guard, "urlopen", side_effect=error) as fetch,
            ):
                with self.assertRaises(HTTPError):
                    guard.fetch_version(PACKAGE["name"], PACKAGE["version"])
                self.assertEqual(fetch.call_count, attempts)

    def test_missing_git_base_is_a_failure_not_an_empty_diff(self):
        with (
            patch.object(guard.sys, "argv", ["check_cargo_cooldown.py"]),
            patch.object(
                guard.subprocess,
                "check_output",
                side_effect=subprocess.CalledProcessError(128, "git"),
            ),
            patch.object(guard, "fetch_version") as fetch,
            patch.object(guard.sys, "stderr", io.StringIO()),
        ):
            self.assertEqual(guard.main(), 1)
            fetch.assert_not_called()


if __name__ == "__main__":
    unittest.main()
