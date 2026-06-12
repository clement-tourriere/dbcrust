"""Pytest configuration for DBCrust Django tests."""

import tempfile

import django
from django.conf import settings


def pytest_configure():
    """Configure minimal Django settings for tests."""
    if not settings.configured:
        settings.configure(
            DEBUG=True,
            ALLOWED_HOSTS=["testserver", "localhost", "127.0.0.1"],
            # Dashboard persistence resolves its default path from BASE_DIR —
            # keep test artifacts (.dbcrust/) out of the repo
            BASE_DIR=tempfile.mkdtemp(prefix="dbcrust-django-tests-"),
            DATABASES={
                "default": {
                    "ENGINE": "django.db.backends.sqlite3",
                    "NAME": ":memory:",
                }
            },
            INSTALLED_APPS=[
                "django.contrib.contenttypes",
                "django.contrib.auth",
                "dbcrust.django",  # template discovery for the dashboard
            ],
            MIDDLEWARE=[],
            SECRET_KEY="test-secret-key-for-dbcrust-tests",
            DEFAULT_AUTO_FIELD="django.db.models.BigAutoField",
            ROOT_URLCONF="dbcrust.django.tests.urls",
            TEMPLATES=[
                {
                    "BACKEND": "django.template.backends.django.DjangoTemplates",
                    "APP_DIRS": True,
                    "OPTIONS": {},
                }
            ],
        )
        django.setup()
