"""
Django management command to run comprehensive DBCrust analysis.

This command allows you to see complete analysis results without truncation.
"""

from django.core.management.base import BaseCommand, CommandError
from django.db import transaction
import logging


class Command(BaseCommand):
    help = 'Run DBCrust performance analysis on a code block'

    def add_arguments(self, parser):
        parser.add_argument(
            '--query',
            type=str,
            help='Raw SQL query to analyze'
        )
        parser.add_argument(
            '--model-query', 
            type=str,
            help='Django ORM query code to analyze (e.g., "User.objects.all()")'
        )
        parser.add_argument(
            '--code-file',
            type=str,
            help='Path to Python file to analyze'
        )
        parser.add_argument(
            '--all-issues',
            action='store_true',
            help='Show all detected issues without limit'
        )
        parser.add_argument(
            '--verbose',
            action='store_true',
            help='Show detailed output including recommendations'
        )

    def handle(self, *args, **options):
        try:
            from dbcrust.django import analyzer
        except ImportError:
            raise CommandError('DBCrust Django analyzer not available')

        self.stdout.write('🔍 DBCrust Performance Analysis')
        self.stdout.write('=' * 50)

        if options['model_query']:
            self._analyze_model_query(options['model_query'], options)
        elif options['code_file']:
            self._analyze_code_file(options['code_file'], options)
        else:
            self._show_usage_examples()

    def _analyze_model_query(self, query_code, options):
        """Analyze a Django ORM query string."""
        self.stdout.write(f'\n📊 Analyzing query: {query_code}')
        self.stdout.write('-' * 30)
        
        try:
            from dbcrust.django import analyzer
            
            with analyzer.analyze() as analysis:
                # Execute the query code safely
                try:
                    exec(f"result = {query_code}")
                except Exception as e:
                    self.stdout.write(f'❌ Query execution failed: {e}')
                    return

            results = analysis.get_results()
            self._display_results(results, options)

        except Exception as e:
            self.stdout.write(f'❌ Analysis failed: {e}')

    def _analyze_code_file(self, file_path, options):
        """Analyze a Python file for Django ORM patterns."""
        self.stdout.write(f'\n📄 Analyzing file: {file_path}')
        self.stdout.write('-' * 30)
        
        try:
            from dbcrust.django.code_analyzer import DjangoCodeAnalyzer
            
            code_analyzer = DjangoCodeAnalyzer('.')
            issues = code_analyzer.analyze_file(file_path)
            
            if not issues:
                self.stdout.write('✅ No issues found in file')
                return
                
            self.stdout.write(f'🔍 Found {len(issues)} potential issues:')
            for i, issue in enumerate(issues, 1):
                self.stdout.write(f'\n{i}. {issue.pattern_type}')
                self.stdout.write(f'   Line {issue.line_number}: {issue.description}')
                if options['verbose'] and hasattr(issue, 'recommendation'):
                    self.stdout.write(f'   💡 {issue.recommendation}')
                    
        except ImportError:
            self.stdout.write('❌ Code analysis not available (requires ENABLE_CODE_ANALYSIS=True)')
        except Exception as e:
            self.stdout.write(f'❌ File analysis failed: {e}')

    def _display_results(self, results, options):
        """Display analysis results."""
        if not results:
            self.stdout.write('❌ No results available')
            return
            
        # Basic metrics
        self.stdout.write(f'\n📈 Query Analysis:')
        self.stdout.write(f'  Total Queries: {results.total_queries}')
        self.stdout.write(f'  Total Duration: {results.total_duration * 1000:.1f}ms')
        self.stdout.write(f'  Duplicate Queries: {results.duplicate_queries}')
        
        # Query types
        if results.queries_by_type:
            self.stdout.write(f'\n📊 Query Types:')
            for query_type, count in results.queries_by_type.items():
                self.stdout.write(f'  {query_type}: {count}')
        
        # Issues
        if results.detected_patterns:
            issue_count = len(results.detected_patterns)
            self.stdout.write(f'\n⚠️ Performance Issues ({issue_count}):')
            
            # Group by severity
            critical = [p for p in results.detected_patterns if p.severity == 'critical']
            high = [p for p in results.detected_patterns if p.severity == 'high'] 
            medium = [p for p in results.detected_patterns if p.severity == 'medium']
            low = [p for p in results.detected_patterns if p.severity == 'low']
            
            if critical:
                self.stdout.write(f'  🔴 Critical: {len(critical)}')
            if high:
                self.stdout.write(f'  🟡 High: {len(high)}')
            if medium:
                self.stdout.write(f'  🟠 Medium: {len(medium)}')
            if low:
                self.stdout.write(f'  🟢 Low: {len(low)}')
            
            # Show all issues if requested
            max_issues = len(results.detected_patterns) if options['all_issues'] else 10
            
            self.stdout.write(f'\n🔍 Issue Details:')
            for i, issue in enumerate(results.detected_patterns[:max_issues], 1):
                severity_icon = {
                    'critical': '🔴',
                    'high': '🟡', 
                    'medium': '🟠',
                    'low': '🟢'
                }.get(issue.severity, '⚪')
                
                self.stdout.write(f'\n{i}. {severity_icon} {issue.pattern_type} ({issue.severity})')
                self.stdout.write(f'   📝 {issue.description}')
                
                if hasattr(issue, 'code_locations') and issue.code_locations:
                    self.stdout.write(f'   📍 Location: {issue.code_locations[0]}')
                    
                if options['verbose']:
                    if hasattr(issue, 'recommendation') and issue.recommendation:
                        self.stdout.write(f'   💡 Recommendation: {issue.recommendation}')
                    if hasattr(issue, 'estimated_impact') and issue.estimated_impact:
                        self.stdout.write(f'   📊 Impact: {issue.estimated_impact}')
            
            remaining = len(results.detected_patterns) - max_issues
            if remaining > 0:
                self.stdout.write(f'\n... and {remaining} more issues (use --all-issues to see all)')
        else:
            self.stdout.write('\n✅ No performance issues detected!')
            
        # Recommendations
        if hasattr(results, 'recommendations') and results.recommendations:
            self.stdout.write(f'\n💡 Top Recommendations:')
            for i, rec in enumerate(results.recommendations[:5], 1):
                self.stdout.write(f'{i}. {rec.title}')
                if options['verbose']:
                    self.stdout.write(f'   {rec.description}')

    def _show_usage_examples(self):
        """Show usage examples."""
        self.stdout.write('\n📖 Usage Examples:')
        self.stdout.write('=' * 30)
        
        examples = [
            ('Analyze a simple query:', 
             'python manage.py dbcrust_analyze --model-query "User.objects.all()"'),
            ('Analyze with joins:', 
             'python manage.py dbcrust_analyze --model-query "Book.objects.select_related(\'author\')"'),
            ('Show all issues:', 
             'python manage.py dbcrust_analyze --model-query "User.objects.all()" --all-issues'),
            ('Verbose output:', 
             'python manage.py dbcrust_analyze --model-query "User.objects.all()" --verbose'),
            ('Analyze a Python file:', 
             'python manage.py dbcrust_analyze --code-file myapp/views.py --all-issues'),
        ]
        
        for desc, cmd in examples:
            self.stdout.write(f'\n• {desc}')
            self.stdout.write(f'  {cmd}')
            
        self.stdout.write(f'\n💡 Pro tip: Use --all-issues --verbose to see complete analysis with recommendations!')