"""Test URLconf: mounts the dashboard the way a host project would."""

from django.urls import include, path

urlpatterns = [
    path("__dbcrust__/", include("dbcrust.django.urls")),
]
