"""
AST-based code analyzer for Django ORM patterns.

Analyzes Python source code to detect Django ORM performance issues
and provides accurate line numbers and code context.
"""

import ast
import os
import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import List, Dict, Set, Optional, Tuple, Any
from collections import defaultdict


@dataclass
class CodeIssue:
    """Represents a code issue found through AST analysis."""
    file_path: str
    line_number: int
    column: int
    issue_type: str
    severity: str  # critical, high, medium, low
    description: str
    code_snippet: str
    suggestion: str
    django_model: Optional[str] = None
    affected_field: Optional[str] = None
    
    @property
    def location(self) -> str:
        """Return file:line format for easy navigation."""
        return f"{self.file_path}:{self.line_number}"


class DjangoORMVisitor(ast.NodeVisitor):
    """AST visitor that detects Django ORM patterns."""
    
    def __init__(self, file_path: str, source_lines: List[str]):
        self.file_path = file_path
        self.source_lines = source_lines
        self.issues: List[CodeIssue] = []
        self.current_class = None
        self.current_function = None
        self.loop_depth = 0
        self.in_template_tag = False
        
        # Track variable assignments for data flow analysis
        self.variable_types: Dict[str, str] = {}
        self.queryset_variables: Set[str] = set()
        
    def get_code_snippet(self, node: ast.AST, context_lines: int = 2) -> str:
        """Extract code snippet around the node."""
        if not hasattr(node, 'lineno'):
            return ""
        
        start_line = max(0, node.lineno - context_lines - 1)
        end_line = min(len(self.source_lines), node.lineno + context_lines)
        
        snippet_lines = []
        for i in range(start_line, end_line):
            prefix = ">>> " if i == node.lineno - 1 else "    "
            snippet_lines.append(f"{prefix}{self.source_lines[i].rstrip()}")
        
        return "\n".join(snippet_lines)
    
    def visit_For(self, node: ast.For) -> None:
        """Detect patterns in for loops."""
        self.loop_depth += 1
        
        # Check if iterating over a queryset
        if isinstance(node.iter, ast.Call):
            self._check_queryset_iteration(node)
        elif isinstance(node.iter, ast.Name) and node.iter.id in self.queryset_variables:
            self._check_queryset_iteration(node)
        
        # Check for queries inside loops
        self._check_queries_in_loop(node)
        
        self.generic_visit(node)
        self.loop_depth -= 1
    
    def visit_ListComp(self, node: ast.ListComp) -> None:
        """Detect patterns in list comprehensions."""
        # List comprehensions can hide N+1 queries
        for generator in node.generators:
            if isinstance(generator.iter, ast.Call):
                self._check_queryset_iteration_in_comprehension(node, generator)
        
        self.generic_visit(node)
    
    def visit_Call(self, node: ast.Call) -> None:
        """Detect Django ORM method calls."""
        call_str = self._get_call_string(node)
        
        # Check for common Django ORM patterns
        if call_str:
            self._check_orm_call(node, call_str)
            
            # Track queryset variables
            if any(method in call_str for method in ['.objects.', '.filter(', '.all(', '.exclude(']):
                if isinstance(node.func, ast.Attribute):
                    if hasattr(node.func.value, 'id'):
                        self.queryset_variables.add(node.func.value.id)
        
        self.generic_visit(node)
    
    def visit_Assign(self, node: ast.Assign) -> None:
        """Track variable assignments for data flow analysis."""
        # Track queryset assignments
        if isinstance(node.value, ast.Call):
            call_str = self._get_call_string(node.value)
            if call_str and '.objects.' in call_str:
                for target in node.targets:
                    if isinstance(target, ast.Name):
                        self.queryset_variables.add(target.id)
                        self.variable_types[target.id] = 'queryset'
        
        self.generic_visit(node)
    
    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        """Track function context."""
        old_function = self.current_function
        self.current_function = node.name
        self.generic_visit(node)
        self.current_function = old_function
    
    def visit_ClassDef(self, node: ast.ClassDef) -> None:
        """Track class context for models."""
        old_class = self.current_class
        self.current_class = node.name
        
        # Check if this is a Django model
        if self._is_django_model(node):
            self._check_model_definition(node)
        
        self.generic_visit(node)
        self.current_class = old_class
    
    def _is_django_model(self, node: ast.ClassDef) -> bool:
        """Check if class is a Django model."""
        for base in node.bases:
            if isinstance(base, ast.Attribute):
                if base.attr == 'Model':
                    return True
            elif isinstance(base, ast.Name):
                if 'Model' in base.id:
                    return True
        return False
    
    def _check_model_definition(self, node: ast.ClassDef):
        """Check Django model definition for optimization opportunities."""
        has_meta = False
        indexed_fields = set()
        foreign_keys = []
        many_to_many = []
        
        for item in node.body:
            # Check for Meta class
            if isinstance(item, ast.ClassDef) and item.name == 'Meta':
                has_meta = True
                self._analyze_meta_class(item, indexed_fields)
            
            # Check field definitions
            elif isinstance(item, ast.Assign):
                for target in item.targets:
                    if isinstance(target, ast.Name):
                        field_name = target.id
                        if isinstance(item.value, ast.Call):
                            call_str = self._get_call_string(item.value)
                            if call_str:
                                if 'ForeignKey' in call_str:
                                    foreign_keys.append(field_name)
                                elif 'ManyToManyField' in call_str:
                                    many_to_many.append(field_name)
                                
                                # Check for db_index
                                if 'db_index=True' in ast.unparse(item.value):
                                    indexed_fields.add(field_name)
        
        # Suggest indexes for commonly filtered fields
        if not has_meta or len(indexed_fields) == 0:
            if foreign_keys or many_to_many:
                self.issues.append(CodeIssue(
                    file_path=self.file_path,
                    line_number=node.lineno,
                    column=node.col_offset,
                    issue_type="missing_model_indexes",
                    severity="medium",
                    description=f"Model {node.name} has relationships but no explicit indexes",
                    code_snippet=self.get_code_snippet(node),
                    suggestion="Consider adding indexes for frequently queried fields:\n\nclass Meta:\n    indexes = [\n        models.Index(fields=['field_name']),\n    ]",
                    django_model=node.name
                ))
    
    def _analyze_meta_class(self, node: ast.ClassDef, indexed_fields: Set[str]):
        """Analyze Meta class for optimization settings."""
        for item in node.body:
            if isinstance(item, ast.Assign):
                for target in item.targets:
                    if isinstance(target, ast.Name):
                        if target.id == 'indexes':
                            # Parse index definitions
                            self._extract_indexed_fields(item.value, indexed_fields)
                        elif target.id == 'index_together':
                            # Legacy index definition
                            self._extract_indexed_fields(item.value, indexed_fields)
    
    def _extract_indexed_fields(self, node: ast.AST, indexed_fields: Set[str]):
        """Extract field names from index definitions."""
        if isinstance(node, ast.List):
            for item in node.elts:
                if isinstance(item, ast.Call):
                    # models.Index(fields=['field'])
                    for keyword in item.keywords:
                        if keyword.arg == 'fields':
                            if isinstance(keyword.value, ast.List):
                                for field in keyword.value.elts:
                                    if isinstance(field, ast.Constant):
                                        indexed_fields.add(field.value)
    
    def _check_queryset_iteration(self, node: ast.For):
        """Check for N+1 patterns in queryset iteration."""
        # Look for attribute access inside the loop
        for child in ast.walk(node):
            if isinstance(child, ast.Attribute):
                # Check for related object access (e.g., item.author.name)
                if self._is_related_object_access(child):
                    self.issues.append(CodeIssue(
                        file_path=self.file_path,
                        line_number=child.lineno,
                        column=child.col_offset,
                        issue_type="n_plus_one_risk",
                        severity="high",
                        description="Accessing related objects in loop - potential N+1 query",
                        code_snippet=self.get_code_snippet(child),
                        suggestion="Use select_related() or prefetch_related() before the loop:\n\nqueryset = Model.objects.select_related('related_field')\nfor item in queryset:\n    # Now accessing item.related_field won't cause extra queries"
                    ))
    
    def _check_queryset_iteration_in_comprehension(self, node: ast.ListComp, generator: ast.comprehension):
        """Check for N+1 patterns in list comprehensions."""
        # Check the comprehension element for related object access
        if isinstance(node.elt, ast.Attribute):
            if self._is_related_object_access(node.elt):
                self.issues.append(CodeIssue(
                    file_path=self.file_path,
                    line_number=node.lineno,
                    column=node.col_offset,
                    issue_type="n_plus_one_in_comprehension",
                    severity="high",
                    description="List comprehension accessing related objects - N+1 query pattern",
                    code_snippet=self.get_code_snippet(node),
                    suggestion="Prefetch related objects before the comprehension:\n\nqueryset = Model.objects.prefetch_related('related_field')\nresult = [item.related_field for item in queryset]"
                ))
    
    def _check_queries_in_loop(self, node: ast.For):
        """Check for queries executed inside loops."""
        for child in ast.walk(node.body[0] if node.body else node):
            if isinstance(child, ast.Call):
                call_str = self._get_call_string(child)
                if call_str and any(orm_method in call_str for orm_method in [
                    '.objects.', '.filter(', '.get(', '.create(', '.update(', '.delete('
                ]):
                    self.issues.append(CodeIssue(
                        file_path=self.file_path,
                        line_number=child.lineno,
                        column=child.col_offset,
                        issue_type="query_in_loop",
                        severity="critical",
                        description="Database query inside loop - causes multiple queries",
                        code_snippet=self.get_code_snippet(child),
                        suggestion="Move query outside the loop or use bulk operations:\n\n# Fetch all data before loop\ndata = Model.objects.filter(id__in=ids)\n\n# Or use bulk operations\nModel.objects.bulk_create(objects)"
                    ))
    
    def _check_orm_call(self, node: ast.Call, call_str: str):
        """Check specific ORM call patterns."""
        # Check for missing only() on large queries
        if '.objects.all()' in call_str and '.only(' not in call_str:
            self.issues.append(CodeIssue(
                file_path=self.file_path,
                line_number=node.lineno,
                column=node.col_offset,
                issue_type="missing_only",
                severity="low",
                description="Fetching all fields with .all() - consider using .only()",
                code_snippet=self.get_code_snippet(node),
                suggestion="Use .only() to fetch specific fields:\n\nModel.objects.only('id', 'name', 'email')"
            ))
        
        # Check for inefficient count
        if '.count()' in call_str and self.loop_depth > 0:
            self.issues.append(CodeIssue(
                file_path=self.file_path,
                line_number=node.lineno,
                column=node.col_offset,
                issue_type="count_in_loop",
                severity="high",
                description="Using .count() inside a loop",
                code_snippet=self.get_code_snippet(node),
                suggestion="Cache count result outside the loop or use aggregation"
            ))
        
        # Check for missing select_related
        if '.get(' in call_str or '.filter(' in call_str:
            # Look for subsequent related object access
            parent = self._get_parent_node(node)
            if parent and self._has_related_access_after(parent, node):
                self.issues.append(CodeIssue(
                    file_path=self.file_path,
                    line_number=node.lineno,
                    column=node.col_offset,
                    issue_type="missing_select_related",
                    severity="medium",
                    description="Query followed by related object access",
                    code_snippet=self.get_code_snippet(node),
                    suggestion="Add .select_related() to fetch related objects:\n\nModel.objects.select_related('related_field').get(id=pk)"
                ))
        
        # Check for inefficient exists check
        if 'len(' in call_str and '.all()' in call_str:
            self.issues.append(CodeIssue(
                file_path=self.file_path,
                line_number=node.lineno,
                column=node.col_offset,
                issue_type="inefficient_exists",
                severity="medium",
                description="Using len() on queryset for existence check",
                code_snippet=self.get_code_snippet(node),
                suggestion="Use .exists() instead:\n\nif queryset.exists():\n    ..."
            ))
    
    def _is_related_object_access(self, node: ast.Attribute) -> bool:
        """Check if attribute access is likely a related object."""
        # Look for chained attribute access (e.g., obj.related.field)
        if isinstance(node.value, ast.Attribute):
            return True
        
        # Check for foreign key patterns
        if hasattr(node, 'attr'):
            common_related = ['author', 'user', 'category', 'parent', 'owner', 'creator']
            if any(rel in node.attr.lower() for rel in common_related):
                return True
        
        return False
    
    def _get_call_string(self, node: ast.Call) -> Optional[str]:
        """Extract a string representation of a call for pattern matching."""
        try:
            return ast.unparse(node)
        except:
            # Fallback for older Python versions
            return None
    
    def _get_parent_node(self, node: ast.AST) -> Optional[ast.AST]:
        """Get parent node (simplified - would need proper parent tracking)."""
        # This is a simplified version - in production you'd track parents properly
        return None
    
    def _has_related_access_after(self, parent: ast.AST, query_node: ast.Call) -> bool:
        """Check if there's related object access after the query."""
        # Simplified check - look for attribute access patterns
        for node in ast.walk(parent):
            if isinstance(node, ast.Attribute) and node != query_node:
                if self._is_related_object_access(node):
                    return True
        return False


class DjangoCodeAnalyzer:
    """Analyzes Django code for ORM performance issues."""
    
    def __init__(self, project_root: Optional[str] = None):
        self.project_root = Path(project_root) if project_root else Path.cwd()
        self.issues: List[CodeIssue] = []
        
    def analyze_file(self, file_path: str) -> List[CodeIssue]:
        """Analyze a single Python file for Django ORM issues."""
        path = Path(file_path)
        if not path.exists():
            return []
        
        try:
            with open(path, 'r', encoding='utf-8') as f:
                source = f.read()
                source_lines = source.splitlines()
            
            tree = ast.parse(source, filename=str(path))
            visitor = DjangoORMVisitor(str(path), source_lines)
            visitor.visit(tree)
            
            return visitor.issues
            
        except SyntaxError as e:
            print(f"Syntax error in {file_path}: {e}")
            return []
        except Exception as e:
            print(f"Error analyzing {file_path}: {e}")
            return []
    
    def analyze_directory(self, directory: str, exclude_patterns: Optional[List[str]] = None) -> List[CodeIssue]:
        """Analyze all Python files in a directory."""
        dir_path = Path(directory)
        if not dir_path.exists():
            return []
        
        exclude_patterns = exclude_patterns or ['migrations', '__pycache__', 'tests', 'venv', '.git']
        all_issues = []
        
        for py_file in dir_path.rglob('*.py'):
            # Skip excluded directories
            if any(pattern in str(py_file) for pattern in exclude_patterns):
                continue
            
            issues = self.analyze_file(str(py_file))
            all_issues.extend(issues)
        
        return all_issues
    
    def analyze_django_project(self) -> List[CodeIssue]:
        """Analyze an entire Django project."""
        # Look for manage.py to confirm it's a Django project
        manage_py = self.project_root / 'manage.py'
        if not manage_py.exists():
            print("Warning: manage.py not found - may not be a Django project root")
        
        # Common Django app directories
        app_dirs = []
        for item in self.project_root.iterdir():
            if item.is_dir() and (item / 'models.py').exists():
                app_dirs.append(item)
        
        all_issues = []
        for app_dir in app_dirs:
            print(f"Analyzing Django app: {app_dir.name}")
            issues = self.analyze_directory(str(app_dir))
            all_issues.extend(issues)
        
        return all_issues
    
    def analyze_templates(self, template_dir: str) -> List[CodeIssue]:
        """Analyze Django templates for ORM-related issues."""
        template_path = Path(template_dir)
        if not template_path.exists():
            return []
        
        issues = []
        for template_file in template_path.rglob('*.html'):
            template_issues = self._analyze_template(template_file)
            issues.extend(template_issues)
        
        return issues
    
    def _analyze_template(self, template_path: Path) -> List[CodeIssue]:
        """Analyze a single Django template."""
        issues = []
        
        try:
            with open(template_path, 'r', encoding='utf-8') as f:
                content = f.read()
                lines = content.splitlines()
            
            # Look for template patterns that might cause N+1
            for i, line in enumerate(lines, 1):
                # Check for loops with related object access
                if '{%' in line and 'for' in line:
                    # Check following lines for related object access
                    for j in range(i, min(i + 10, len(lines))):
                        if '.' in lines[j] and '{{' in lines[j]:
                            # Potential related object access in template
                            if re.search(r'{{\s*\w+\.\w+\.\w+', lines[j]):
                                issues.append(CodeIssue(
                                    file_path=str(template_path),
                                    line_number=j + 1,
                                    column=0,
                                    issue_type="template_n_plus_one",
                                    severity="high",
                                    description="Related object access in template loop - N+1 risk",
                                    code_snippet='\n'.join(lines[i-1:j+1]),
                                    suggestion="Prefetch related objects in the view:\n\ncontext['items'] = Model.objects.prefetch_related('related_field')"
                                ))
                                break
        
        except Exception as e:
            print(f"Error analyzing template {template_path}: {e}")
        
        return issues
    
    def generate_report(self, issues: List[CodeIssue]) -> str:
        """Generate a report of found issues."""
        if not issues:
            return "No Django ORM issues found!"
        
        report_lines = [
            "Django ORM Code Analysis Report",
            "================================",
            f"Found {len(issues)} potential issues",
            ""
        ]
        
        # Group by severity
        by_severity = defaultdict(list)
        for issue in issues:
            by_severity[issue.severity].append(issue)
        
        for severity in ['critical', 'high', 'medium', 'low']:
            if severity in by_severity:
                report_lines.append(f"\n{severity.upper()} Issues ({len(by_severity[severity])})")
                report_lines.append("-" * 40)
                
                for issue in by_severity[severity]:
                    report_lines.extend([
                        f"\n📍 {issue.location}",
                        f"Type: {issue.issue_type}",
                        f"Description: {issue.description}",
                        f"Code:",
                        issue.code_snippet,
                        f"Suggestion:",
                        issue.suggestion,
                        ""
                    ])
        
        return "\n".join(report_lines)


def analyze_django_code(file_or_dir: str) -> Tuple[List[CodeIssue], str]:
    """
    Main entry point for code analysis.
    
    Args:
        file_or_dir: Path to file, directory, or Django project root
    
    Returns:
        Tuple of (issues list, report string)
    """
    analyzer = DjangoCodeAnalyzer()
    
    path = Path(file_or_dir)
    if path.is_file():
        issues = analyzer.analyze_file(str(path))
    elif path.is_dir():
        # Check if it's a Django project root
        if (path / 'manage.py').exists():
            issues = analyzer.analyze_django_project()
        else:
            issues = analyzer.analyze_directory(str(path))
    else:
        return [], f"Path not found: {file_or_dir}"
    
    report = analyzer.generate_report(issues)
    return issues, report