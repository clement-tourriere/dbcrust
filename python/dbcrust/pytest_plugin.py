"""
Pytest plugin: query-budget fixtures for Django projects.

Auto-registered via the ``pytest11`` entry point when dbcrust is installed.
Everything Django-related is imported lazily inside the fixture, so the
plugin is inert for non-Django test suites.

Usage::

    def test_dashboard(client, dbcrust):
        with dbcrust.max_queries(10):
            client.get("/dashboard/")

    def test_book_list(dbcrust):
        with dbcrust.no_n_plus_one():
            for book in Book.objects.select_related("author"):
                _ = book.author.name
"""

import pytest


class DbcrustQueryHelpers:
    """Pre-bound access to :mod:`dbcrust.django.testing`."""

    def __init__(self, using: str = "default"):
        self.using = using

    def max_queries(self, count: int, using: str = None):
        from dbcrust.django.testing import assert_max_queries

        return assert_max_queries(count, using=using or self.using)

    def no_n_plus_one(self, using: str = None):
        from dbcrust.django.testing import assert_no_n_plus_one

        return assert_no_n_plus_one(using=using or self.using)

    def capture(self, using: str = None):
        from dbcrust.django.testing import capture_queries

        return capture_queries(using=using or self.using)


@pytest.fixture
def dbcrust() -> DbcrustQueryHelpers:
    """Query-budget assertion helpers bound to the default database."""
    return DbcrustQueryHelpers()
