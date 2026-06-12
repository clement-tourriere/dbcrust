"""
Storage backing the DBCrust performance dashboard.

The middleware records one :class:`RequestRecord` per analyzed request; the
dashboard views read them back. Two backends share the same interface:

- :class:`SqliteDashboardStore` (default) — a dedicated SQLite file
  (``.dbcrust/dashboard.sqlite3`` next to ``BASE_DIR``), so history survives
  dev-server autoreloads and restarts, and gunicorn workers share one
  timeline. This is NOT your project database: no models, no migrations,
  nothing to configure.
- :class:`DashboardStore` — in-memory ring buffer, used when
  ``DASHBOARD_PERSIST`` is ``False``. Zero filesystem footprint, history
  dies with the process.

Records are capped (``DASHBOARD_MAX_REQUESTS``) in both backends. History is
disposable by design: on schema changes the SQLite table is dropped and
recreated, and rows that no longer deserialize are skipped.
"""

import dataclasses
import itertools
import json
import logging
import sqlite3
import threading
from collections import deque
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Dict, List, Optional, Union

from .report_formatter import IssueInfo, RequestPerformanceReport, SlowQueryInfo

logger = logging.getLogger("dbcrust.performance")

DEFAULT_MAX_REQUESTS = 100

#: Bumped when the stored shape changes; old dashboard files are dropped and
#: recreated (history is a cache, not precious data).
_SCHEMA_VERSION = 1

#: Map report grades to a coarse CSS class used by the templates.
_GRADE_CLASSES = {
    "A": "good",
    "B": "good",
    "C": "warn",
    "D": "bad",
    "F": "bad",
}


@dataclass
class RequestRecord:
    """One analyzed request as shown in the dashboard."""

    id: int
    captured_at: datetime
    report: RequestPerformanceReport

    @property
    def critical_count(self) -> int:
        return len(self.report.critical_issues)

    @property
    def warning_count(self) -> int:
        return len(self.report.warnings)

    @property
    def hint_count(self) -> int:
        return len(self.report.hints)

    @property
    def total_issues(self) -> int:
        return self.critical_count + self.warning_count + self.hint_count

    @property
    def grade_class(self) -> str:
        return _GRADE_CLASSES.get(self.report.grade, "warn")


def _stats_from_records(records: List[RequestRecord]) -> Dict[str, object]:
    """Aggregates for the dashboard header."""
    count = len(records)
    if count == 0:
        return {
            "request_count": 0,
            "with_issues": 0,
            "total_queries": 0,
            "avg_queries": 0.0,
            "avg_db_time_ms": 0.0,
        }
    total_queries = sum(r.report.total_queries for r in records)
    return {
        "request_count": count,
        "with_issues": sum(1 for r in records if r.total_issues > 0),
        "total_queries": total_queries,
        "avg_queries": total_queries / count,
        "avg_db_time_ms": sum(r.report.db_time_ms for r in records) / count,
    }


# ---------------------------------------------------------------------------
# Report (de)serialization — used by the SQLite backend
# ---------------------------------------------------------------------------

_REPORT_FIELDS = {f.name for f in dataclasses.fields(RequestPerformanceReport)}


def _serialize_report(report: RequestPerformanceReport) -> str:
    return json.dumps(dataclasses.asdict(report))


def _deserialize_report(payload: str) -> RequestPerformanceReport:
    data = json.loads(payload)
    # Drop fields a newer/older version may have added — best effort restore.
    data = {k: v for k, v in data.items() if k in _REPORT_FIELDS}
    for key in ("critical_issues", "warnings", "hints"):
        data[key] = [IssueInfo(**issue) for issue in data.get(key, [])]
    data["slow_queries"] = [SlowQueryInfo(**q) for q in data.get("slow_queries", [])]
    return RequestPerformanceReport(**data)


# ---------------------------------------------------------------------------
# In-memory backend
# ---------------------------------------------------------------------------


class DashboardStore:
    """Thread-safe in-memory ring buffer of :class:`RequestRecord` entries."""

    def __init__(self, max_entries: int = DEFAULT_MAX_REQUESTS):
        self._lock = threading.Lock()
        self._entries: deque = deque(maxlen=max(1, int(max_entries)))
        self._ids = itertools.count(1)

    def set_max_entries(self, max_entries: int) -> None:
        """Resize the buffer, keeping the most recent entries."""
        max_entries = max(1, int(max_entries))
        with self._lock:
            if self._entries.maxlen == max_entries:
                return
            self._entries = deque(self._entries, maxlen=max_entries)

    def add(self, report: RequestPerformanceReport) -> RequestRecord:
        record = RequestRecord(
            id=next(self._ids),
            captured_at=datetime.now(timezone.utc),
            report=report,
        )
        with self._lock:
            self._entries.append(record)
        return record

    def records(self) -> List[RequestRecord]:
        """All records, newest first."""
        with self._lock:
            return list(reversed(self._entries))

    def get(self, record_id: int) -> Optional[RequestRecord]:
        with self._lock:
            for record in self._entries:
                if record.id == record_id:
                    return record
        return None

    def clear(self) -> None:
        with self._lock:
            self._entries.clear()

    def stats(self) -> Dict[str, object]:
        return _stats_from_records(self.records())


# ---------------------------------------------------------------------------
# SQLite backend
# ---------------------------------------------------------------------------


class SqliteDashboardStore:
    """
    :class:`DashboardStore`-compatible backend persisted to its own SQLite
    file. Survives autoreloads/restarts and is shared across worker
    processes (WAL journal, short busy timeout, one connection per
    operation).
    """

    def __init__(self, db_path: Union[str, Path], max_entries: int = DEFAULT_MAX_REQUESTS):
        self._db_path = Path(db_path)
        self._max_entries = max(1, int(max_entries))
        self._db_path.parent.mkdir(parents=True, exist_ok=True)
        with self._connect() as conn:
            version = conn.execute("PRAGMA user_version").fetchone()[0]
            if version != _SCHEMA_VERSION:
                conn.execute("DROP TABLE IF EXISTS requests")
                conn.execute(f"PRAGMA user_version = {_SCHEMA_VERSION}")
            # WAL lets the polling dashboard read while workers write.
            # Fails harmlessly (e.g. network FS) — sqlite falls back.
            conn.execute("PRAGMA journal_mode = WAL")
            conn.execute(
                """
                CREATE TABLE IF NOT EXISTS requests (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    captured_at TEXT NOT NULL,
                    report_json TEXT NOT NULL
                )
                """
            )

    @property
    def db_path(self) -> Path:
        return self._db_path

    def _connect(self) -> sqlite3.Connection:
        return sqlite3.connect(self._db_path, timeout=5)

    def set_max_entries(self, max_entries: int) -> None:
        self._max_entries = max(1, int(max_entries))
        with self._connect() as conn:
            conn.execute(
                "DELETE FROM requests WHERE id NOT IN "
                "(SELECT id FROM requests ORDER BY id DESC LIMIT ?)",
                (self._max_entries,),
            )

    def add(self, report: RequestPerformanceReport) -> RequestRecord:
        captured_at = datetime.now(timezone.utc)
        with self._connect() as conn:
            cursor = conn.execute(
                "INSERT INTO requests (captured_at, report_json) VALUES (?, ?)",
                (captured_at.isoformat(), _serialize_report(report)),
            )
            record_id = cursor.lastrowid
            # AUTOINCREMENT ids are monotonic, so this prunes everything
            # older than the newest max_entries rows.
            conn.execute(
                "DELETE FROM requests WHERE id <= ?",
                (record_id - self._max_entries,),
            )
        return RequestRecord(id=record_id, captured_at=captured_at, report=report)

    def _row_to_record(self, row) -> Optional[RequestRecord]:
        record_id, captured_at, payload = row
        try:
            return RequestRecord(
                id=record_id,
                captured_at=datetime.fromisoformat(captured_at),
                report=_deserialize_report(payload),
            )
        except Exception as e:
            logger.debug("Skipping unreadable dashboard record %s: %s", record_id, e)
            return None

    def records(self) -> List[RequestRecord]:
        """All records, newest first."""
        with self._connect() as conn:
            rows = conn.execute(
                "SELECT id, captured_at, report_json FROM requests "
                "ORDER BY id DESC LIMIT ?",
                (self._max_entries,),
            ).fetchall()
        return [record for record in map(self._row_to_record, rows) if record]

    def get(self, record_id: int) -> Optional[RequestRecord]:
        with self._connect() as conn:
            row = conn.execute(
                "SELECT id, captured_at, report_json FROM requests WHERE id = ?",
                (record_id,),
            ).fetchone()
        return self._row_to_record(row) if row else None

    def clear(self) -> None:
        with self._connect() as conn:
            conn.execute("DELETE FROM requests")

    def stats(self) -> Dict[str, object]:
        return _stats_from_records(self.records())


# ---------------------------------------------------------------------------
# Process-wide store configuration
# ---------------------------------------------------------------------------

_store: Optional[Union[DashboardStore, SqliteDashboardStore]] = None
_store_lock = threading.Lock()


def _default_db_path() -> Path:
    """``<BASE_DIR>/.dbcrust/dashboard.sqlite3`` (cwd when BASE_DIR is unset)."""
    try:
        from django.conf import settings
        base = getattr(settings, "BASE_DIR", None)
    except Exception:
        base = None
    root = Path(base) if base else Path.cwd()
    return root / ".dbcrust" / "dashboard.sqlite3"


def configure_store(
    persist: bool = True,
    db_path: Union[str, Path, None] = None,
    max_entries: int = DEFAULT_MAX_REQUESTS,
) -> Union[DashboardStore, SqliteDashboardStore]:
    """(Re)build the process-wide store. Called by the middleware at startup."""
    global _store
    with _store_lock:
        if persist:
            try:
                _store = SqliteDashboardStore(db_path or _default_db_path(), max_entries)
            except Exception as e:
                logger.warning(
                    "DBCrust dashboard: could not open persistent store (%s) — "
                    "falling back to in-memory history",
                    e,
                )
                _store = DashboardStore(max_entries)
        else:
            _store = DashboardStore(max_entries)
        return _store


def get_store() -> Union[DashboardStore, SqliteDashboardStore]:
    """
    The configured store, building one from ``DBCRUST_PERFORMANCE_ANALYSIS``
    on first use (the views may be hit before the middleware initializes).
    """
    if _store is not None:
        return _store
    try:
        from django.conf import settings
        config = getattr(settings, "DBCRUST_PERFORMANCE_ANALYSIS", {}) or {}
    except Exception:
        config = {}
    return configure_store(
        persist=config.get("DASHBOARD_PERSIST", True),
        db_path=config.get("DASHBOARD_DB_PATH"),
        max_entries=config.get("DASHBOARD_MAX_REQUESTS", DEFAULT_MAX_REQUESTS),
    )
