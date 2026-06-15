"""Background job runner for dashboard AI investigations.

An investigation blocks for many seconds. Running it in a daemon thread — with
the GIL released inside the Rust call (see ``run_ai_investigation``) — keeps the
dashboard responsive. The agent appends its step-by-step progress to a temp file
which the status endpoint tails via htmx polling, so the browser sees the
investigation unfold in real time instead of staring at a frozen spinner.
"""

from __future__ import annotations

import atexit
import os
import tempfile
import threading
import time
from dataclasses import dataclass, field
from typing import Callable, Dict, Optional

# Total jobs retained (finished ones are evicted oldest-first beyond this) and the
# cap on concurrent investigations (each is an expensive AI + DB run).
MAX_JOBS = 30
MAX_RUNNING = 3


@dataclass
class AiJob:
    """One in-flight (or finished) investigation for a dashboard record."""

    key: int
    progress_path: str
    status: str = "running"  # running | done | error
    answer: str = ""
    error: str = ""
    started_at: float = field(default_factory=time.time)
    thread: Optional[threading.Thread] = None

    @property
    def elapsed(self) -> int:
        return int(time.time() - self.started_at)

    def progress_text(self) -> str:
        """The agent's progress narration so far (tailed from the temp file)."""
        try:
            with open(self.progress_path, encoding="utf-8", errors="replace") as fh:
                return fh.read()
        except OSError:
            return ""


class AiJobStore:
    """Process-wide registry of investigations, keyed by dashboard record id."""

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._jobs: Dict[int, AiJob] = {}

    def get(self, key: int) -> Optional[AiJob]:
        with self._lock:
            return self._jobs.get(key)

    @staticmethod
    def _unlink(job: AiJob) -> None:
        if job.progress_path:
            try:
                os.unlink(job.progress_path)
            except OSError:
                pass

    def _prune_locked(self) -> None:
        """Evict oldest FINISHED jobs once over budget (never touch running ones)."""
        if len(self._jobs) <= MAX_JOBS:
            return
        finished = sorted(
            (j for j in self._jobs.values() if j.status != "running"),
            key=lambda j: j.started_at,
        )
        for job in finished[: len(self._jobs) - MAX_JOBS]:
            self._unlink(job)
            self._jobs.pop(job.key, None)

    def start(self, key: int, runner: Callable[[str], str]) -> AiJob:
        """Start (or restart) the investigation for ``key``.

        ``runner(progress_path)`` runs the blocking call and returns the final
        answer; any exception becomes the job's error. A job already ``running``
        for this key is returned as-is (re-clicking is a no-op while in flight).
        Returns a terminal error job (no thread) if too many are already running.
        """
        with self._lock:
            existing = self._jobs.get(key)
            if existing is not None and existing.status == "running":
                return existing

            running = sum(1 for j in self._jobs.values() if j.status == "running")
            if running >= MAX_RUNNING:
                # Don't spawn another expensive job; report a terminal error the
                # status panel renders (no temp file, no thread). Still drop the
                # finished job we're replacing so its temp file isn't orphaned.
                if existing is not None:
                    self._unlink(existing)
                job = AiJob(
                    key=key,
                    progress_path="",
                    status="error",
                    error=f"Too many investigations running ({running}); try again shortly.",
                )
                self._jobs[key] = job
                return job

            if existing is not None:
                # Replacing a finished job for this key — drop its progress file.
                self._unlink(existing)
            fd, path = tempfile.mkstemp(prefix="dbcrust_ai_", suffix=".log")
            os.close(fd)
            job = AiJob(key=key, progress_path=path)
            self._jobs[key] = job
            self._prune_locked()

        def _run() -> None:
            try:
                job.answer = runner(job.progress_path)
                job.status = "done"
            except Exception as exc:  # noqa: BLE001 — surface any failure to the UI
                job.error = str(exc)
                job.status = "error"

        job.thread = threading.Thread(
            target=_run, name=f"dbcrust-ai-{key}", daemon=True
        )
        job.thread.start()
        return job

    def clear(self) -> None:
        """Drop FINISHED jobs and their temp files; running jobs are left alone.

        Called from the dashboard's Clear so it doesn't yank a live investigation.
        """
        with self._lock:
            for key in [k for k, j in self._jobs.items() if j.status != "running"]:
                self._unlink(self._jobs.pop(key))

    def _cleanup_all(self) -> None:
        """Unlink every temp file (process exit — running jobs die with it)."""
        with self._lock:
            for job in self._jobs.values():
                self._unlink(job)
            self._jobs.clear()


_store: Optional[AiJobStore] = None


def get_job_store() -> AiJobStore:
    global _store
    if _store is None:
        _store = AiJobStore()
    return _store


@atexit.register
def _cleanup_temp_files() -> None:
    if _store is not None:
        _store._cleanup_all()
