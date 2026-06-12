"""
In-memory store backing the DBCrust performance dashboard.

The middleware records one :class:`RequestRecord` per analyzed request into a
process-local ring buffer; the dashboard views read from it. Nothing is ever
written to disk or sent anywhere.

Notes:
    - The buffer is per-process. Under multi-process servers (gunicorn with
      several workers) each worker keeps its own history; with ``runserver``
      — the intended use case — there is exactly one.
    - Thread-safe: entries are guarded by a lock so threaded dev servers and
      the polling dashboard views can't race.
"""

import itertools
import threading
from collections import deque
from dataclasses import dataclass
from datetime import datetime, timezone
from typing import Dict, List, Optional

from .report_formatter import RequestPerformanceReport

DEFAULT_MAX_REQUESTS = 100

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


class DashboardStore:
    """Thread-safe ring buffer of :class:`RequestRecord` entries."""

    def __init__(self, max_entries: int = DEFAULT_MAX_REQUESTS):
        self._lock = threading.Lock()
        self._entries: deque = deque(maxlen=max_entries)
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
        """Aggregates for the dashboard header."""
        with self._lock:
            records = list(self._entries)
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


#: Process-wide store shared by the middleware and the dashboard views.
store = DashboardStore()
