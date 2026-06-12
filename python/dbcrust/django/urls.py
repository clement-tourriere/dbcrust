"""
URLconf for the DBCrust performance dashboard.

Mount under any prefix, gated on DEBUG::

    # urls.py
    from django.conf import settings
    from django.urls import include, path

    if settings.DEBUG:
        urlpatterns += [path('__dbcrust__/', include('dbcrust.django.urls'))]

The ``app_name`` namespace is also how the middleware recognizes (and skips
analyzing) the dashboard's own polling requests.
"""

from django.urls import path

from . import views

app_name = "dbcrust"

urlpatterns = [
    path("", views.index, name="index"),
    path("requests/", views.request_list, name="request-list"),
    path("requests/<int:record_id>/", views.request_detail, name="request-detail"),
    path("clear/", views.clear, name="clear"),
    path("htmx.min.js", views.htmx_js, name="htmx"),
]
