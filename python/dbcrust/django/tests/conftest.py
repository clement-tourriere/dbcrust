"""Pytest configuration for DBCrust Django tests."""

import django
from django.conf import settings


def pytest_configure():
    """Configure minimal Django settings for tests."""
    if not settings.configured:
        settings.configure(
            DEBUG=True,
            DATABASES={
                "default": {
                    "ENGINE": "django.db.backends.sqlite3",
                    "NAME": ":memory:",
                }
            },
            INSTALLED_APPS=[
                "django.contrib.contenttypes",
                "django.contrib.auth",
            ],
            MIDDLEWARE=[],
            SECRET_KEY="test-secret-key-for-dbcrust-tests",
            DEFAULT_AUTO_FIELD="django.db.models.BigAutoField",
        )
        django.setup()
