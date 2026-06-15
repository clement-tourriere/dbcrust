"""
Django management command: ask the DBCrust AI about your database, with Django
model + ORM context.

The AI investigates the live database read-only (listing tables, describing them,
running EXPLAIN/SELECT) and, because it is handed your Django models, recommends
Django-level fixes (select_related / prefetch_related / db_index / Meta.indexes)
with file:line references — not just raw SQL.

Usage:
    python manage.py dbcrust_ai "why is the order list view slow?"
    python manage.py dbcrust_ai "which tables lack an index on their FK?" --database analytics
"""

from django.core.management.base import BaseCommand, CommandError


class Command(BaseCommand):
    help = "Ask the DBCrust AI a question about your database, with Django model context."

    def add_arguments(self, parser):
        parser.add_argument("question", type=str, help="The question to investigate")
        parser.add_argument(
            "--database",
            type=str,
            default="default",
            help="Django database alias from DATABASES (default: 'default')",
        )
        parser.add_argument(
            "--no-agentic",
            action="store_true",
            help="Single-shot generation instead of the agentic investigation loop",
        )
        parser.add_argument(
            "--max-iterations",
            type=int,
            default=None,
            help="Override the agent's maximum tool-call turns",
        )

    def handle(self, *args, **options):
        try:
            from dbcrust.django.ai_context import ask_ai
        except ImportError as e:
            raise CommandError(f"DBCrust Django AI not available: {e}")

        from django.conf import settings

        project_root = str(getattr(settings, "BASE_DIR", "") or "") or None

        self.stdout.write(self.style.MIGRATE_HEADING("🔍 DBCrust AI"))
        try:
            answer = ask_ai(
                options["question"],
                database=options["database"],
                project_root=project_root,
                agentic=not options["no_agentic"],
                max_iterations=options["max_iterations"],
                # Console command — stream the agent's tool trace as it works.
                stdout_progress=True,
            )
        except Exception as e:
            raise CommandError(f"AI investigation failed: {e}")

        self.stdout.write("")
        self.stdout.write(answer)
