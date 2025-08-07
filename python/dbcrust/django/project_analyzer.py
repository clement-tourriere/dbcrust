"""
Django project-wide ORM analyzer.

Provides comprehensive analysis of entire Django projects, scanning models,
views, and templates for ORM optimization opportunities.
"""

import os
import re
import ast
import subprocess
from pathlib import Path
from typing import List, Dict, Set, Optional, Tuple, Any
from collections import defaultdict
from dataclasses import dataclass, field

from .code_analyzer import DjangoCodeAnalyzer, CodeIssue
from .query_plan_analyzer import PostgreSQLPlanAnalyzer, OptimizationSuggestion


@dataclass
class DjangoModel:
    """Represents a Django model with its fields and relationships."""
    name: str
    file_path: str
    line_number: int
    fields: Dict[str, str] = field(default_factory=dict)  # field_name -> field_type
    foreign_keys: Dict[str, str] = field(default_factory=dict)  # field_name -> related_model
    many_to_many: Dict[str, str] = field(default_factory=dict)  # field_name -> related_model
    indexes: List[str] = field(default_factory=list)
    meta_options: Dict[str, Any] = field(default_factory=dict)
    
    @property
    def table_name(self) -> str:
        """Get the database table name for this model."""
        if 'db_table' in self.meta_options:
            return self.meta_options['db_table']
        else:
            # Convert CamelCase to snake_case
            return re.sub(r'(?<!^)(?=[A-Z])', '_', self.name).lower()


@dataclass
class ProjectAnalysisResult:
    """Results from project-wide analysis."""
    models: List[DjangoModel]
    code_issues: List[CodeIssue]
    model_relationships: Dict[str, List[str]]  # model -> related models
    optimization_score: float  # 0-100
    summary: str
    recommendations: List[str]


class DjangoProjectAnalyzer:
    """Analyzes entire Django projects for ORM optimization opportunities."""
    
    def __init__(self, project_root: str):
        self.project_root = Path(project_root)
        self.models: Dict[str, DjangoModel] = {}
        self.apps: List[Path] = []
        self.code_analyzer = DjangoCodeAnalyzer(str(project_root))
        
        # Validate Django project
        if not (self.project_root / 'manage.py').exists():
            raise ValueError(f"Not a Django project root: {project_root}")
        
        # Find Django apps
        self._discover_apps()
    
    def analyze_project(self) -> ProjectAnalysisResult:
        """
        Perform comprehensive project analysis.
        
        Returns:
            ProjectAnalysisResult with all findings
        """
        print(f"🔍 Analyzing Django project at {self.project_root}")
        
        # 1. Parse all models
        models = self._analyze_models()
        
        # 2. Analyze code for ORM patterns
        code_issues = self._analyze_code_patterns()
        
        # 3. Analyze model relationships
        relationships = self._analyze_model_relationships()
        
        # 4. Generate optimization recommendations
        recommendations = self._generate_project_recommendations(models, code_issues, relationships)
        
        # 5. Calculate optimization score
        score = self._calculate_optimization_score(code_issues, models)
        
        # 6. Generate summary
        summary = self._generate_project_summary(models, code_issues, relationships, score)
        
        return ProjectAnalysisResult(
            models=list(models.values()),
            code_issues=code_issues,
            model_relationships=relationships,
            optimization_score=score,
            summary=summary,
            recommendations=recommendations
        )
    
    def analyze_models_only(self) -> List[DjangoModel]:
        """Analyze only Django models without full project analysis."""
        return self._analyze_models()
    
    def suggest_indexes(self) -> Dict[str, List[str]]:
        """
        Suggest database indexes based on model analysis.
        
        Returns:
            Dict mapping model names to suggested index field lists
        """
        suggestions = {}
        
        for model in self.models.values():
            suggested_indexes = []
            
            # Foreign keys should have indexes (Django adds them automatically)
            for fk_field in model.foreign_keys.keys():
                if fk_field not in model.indexes:
                    suggested_indexes.append([fk_field])
            
            # Common patterns that need indexes
            common_patterns = ['slug', 'status', 'is_active', 'created_at', 'modified_at']
            for field_name, field_type in model.fields.items():
                if any(pattern in field_name.lower() for pattern in common_patterns):
                    if field_name not in model.indexes:
                        suggested_indexes.append([field_name])
            
            if suggested_indexes:
                suggestions[model.name] = suggested_indexes
        
        return suggestions
    
    def find_n_plus_one_risks(self) -> List[CodeIssue]:
        """Find potential N+1 query patterns in the codebase."""
        n_plus_one_issues = []
        
        # Analyze views and other Python files
        for app_dir in self.apps:
            for py_file in app_dir.rglob('*.py'):
                if py_file.name in ['models.py', '__init__.py']:
                    continue
                
                issues = self.code_analyzer.analyze_file(str(py_file))
                n_plus_one_issues.extend([
                    issue for issue in issues 
                    if 'n_plus_one' in issue.issue_type
                ])
        
        return n_plus_one_issues
    
    def analyze_templates(self) -> List[CodeIssue]:
        """Analyze Django templates for ORM issues."""
        template_issues = []
        
        # Look for template directories
        for app_dir in self.apps:
            template_dirs = [
                app_dir / 'templates',
                self.project_root / 'templates'
            ]
            
            for template_dir in template_dirs:
                if template_dir.exists():
                    issues = self.code_analyzer.analyze_templates(str(template_dir))
                    template_issues.extend(issues)
        
        return template_issues
    
    def generate_migration_suggestions(self) -> List[str]:
        """Generate Django migrations for suggested indexes."""
        suggestions = []
        index_suggestions = self.suggest_indexes()
        
        for model_name, indexes in index_suggestions.items():
            model = self.models.get(model_name)
            if model:
                app_name = self._get_app_name_for_model(model)
                migration_code = self._generate_migration_code(model_name, indexes)
                
                suggestions.append(f"""
# Create migration for {model_name} indexes:
# python manage.py makemigrations {app_name} --empty
# Add this to the migration file:

{migration_code}
""")
        
        return suggestions
    
    def _discover_apps(self):
        """Discover Django apps in the project."""
        # Look for directories with models.py
        for item in self.project_root.iterdir():
            if item.is_dir() and (item / 'models.py').exists():
                self.apps.append(item)
                print(f"📦 Found Django app: {item.name}")
        
        if not self.apps:
            print("⚠️  No Django apps found with models.py")
    
    def _analyze_models(self) -> Dict[str, DjangoModel]:
        """Analyze all Django models in the project."""
        models = {}
        
        for app_dir in self.apps:
            models_file = app_dir / 'models.py'
            if models_file.exists():
                app_models = self._parse_models_file(str(models_file))
                models.update(app_models)
                print(f"📋 Found {len(app_models)} models in {app_dir.name}")
        
        self.models = models
        return models
    
    def _parse_models_file(self, file_path: str) -> Dict[str, DjangoModel]:
        """Parse a models.py file to extract model definitions."""
        models = {}
        
        try:
            with open(file_path, 'r', encoding='utf-8') as f:
                source = f.read()
            
            tree = ast.parse(source, filename=file_path)
            
            for node in ast.walk(tree):
                if isinstance(node, ast.ClassDef) and self._is_django_model(node):
                    model = self._parse_model_class(node, file_path)
                    models[model.name] = model
        
        except Exception as e:
            print(f"Error parsing {file_path}: {e}")
        
        return models
    
    def _is_django_model(self, node: ast.ClassDef) -> bool:
        """Check if a class definition is a Django model."""
        for base in node.bases:
            if isinstance(base, ast.Attribute):
                if base.attr == 'Model':
                    return True
            elif isinstance(base, ast.Name):
                if 'Model' in base.id:
                    return True
        return False
    
    def _parse_model_class(self, node: ast.ClassDef, file_path: str) -> DjangoModel:
        """Parse a Django model class definition."""
        model = DjangoModel(
            name=node.name,
            file_path=file_path,
            line_number=node.lineno
        )
        
        for item in node.body:
            if isinstance(item, ast.Assign):
                # Parse field definitions
                for target in item.targets:
                    if isinstance(target, ast.Name):
                        field_name = target.id
                        field_info = self._parse_field_definition(item.value)
                        
                        if field_info['type']:
                            model.fields[field_name] = field_info['type']
                            
                            if field_info['type'] == 'ForeignKey':
                                model.foreign_keys[field_name] = field_info.get('related_model', 'Unknown')
                            elif field_info['type'] == 'ManyToManyField':
                                model.many_to_many[field_name] = field_info.get('related_model', 'Unknown')
                            
                            if field_info.get('db_index'):
                                model.indexes.append(field_name)
            
            elif isinstance(item, ast.ClassDef) and item.name == 'Meta':
                # Parse Meta class
                meta_options = self._parse_meta_class(item)
                model.meta_options.update(meta_options)
                
                # Extract index information
                if 'indexes' in meta_options:
                    model.indexes.extend(meta_options['indexes'])
        
        return model
    
    def _parse_field_definition(self, node: ast.AST) -> Dict[str, Any]:
        """Parse a Django model field definition."""
        result = {'type': None, 'related_model': None, 'db_index': False}
        
        if isinstance(node, ast.Call):
            if isinstance(node.func, ast.Name):
                result['type'] = node.func.id
            elif isinstance(node.func, ast.Attribute):
                result['type'] = node.func.attr
            
            # Parse field arguments
            for arg in node.args:
                if isinstance(arg, ast.Name):
                    result['related_model'] = arg.id
                elif isinstance(arg, ast.Constant) and isinstance(arg.value, str):
                    result['related_model'] = arg.value
            
            # Parse keyword arguments
            for keyword in node.keywords:
                if keyword.arg == 'db_index' and isinstance(keyword.value, ast.Constant):
                    result['db_index'] = keyword.value
        
        return result
    
    def _parse_meta_class(self, node: ast.ClassDef) -> Dict[str, Any]:
        """Parse a Django model Meta class."""
        meta_options = {}
        
        for item in node.body:
            if isinstance(item, ast.Assign):
                for target in item.targets:
                    if isinstance(target, ast.Name):
                        option_name = target.id
                        
                        if option_name == 'db_table' and isinstance(item.value, ast.Constant):
                            meta_options['db_table'] = item.value.value
                        
                        elif option_name == 'indexes':
                            # Parse index definitions
                            indexes = self._parse_index_definitions(item.value)
                            meta_options['indexes'] = indexes
        
        return meta_options
    
    def _parse_index_definitions(self, node: ast.AST) -> List[str]:
        """Parse index field definitions from Meta.indexes."""
        indexes = []
        
        if isinstance(node, ast.List):
            for item in node.elts:
                if isinstance(item, ast.Call):
                    # models.Index(fields=['field1', 'field2'])
                    for keyword in item.keywords:
                        if keyword.arg == 'fields' and isinstance(keyword.value, ast.List):
                            for field_node in keyword.value.elts:
                                if isinstance(field_node, ast.Constant):
                                    indexes.append(field_node.value)
        
        return indexes
    
    def _analyze_code_patterns(self) -> List[CodeIssue]:
        """Analyze code patterns across the project."""
        all_issues = []
        
        # Analyze Python files
        for app_dir in self.apps:
            for py_file in app_dir.rglob('*.py'):
                if 'migrations' in str(py_file) or '__pycache__' in str(py_file):
                    continue
                
                issues = self.code_analyzer.analyze_file(str(py_file))
                all_issues.extend(issues)
        
        # Analyze templates
        template_issues = self.analyze_templates()
        all_issues.extend(template_issues)
        
        return all_issues
    
    def _analyze_model_relationships(self) -> Dict[str, List[str]]:
        """Analyze relationships between models."""
        relationships = defaultdict(list)
        
        for model_name, model in self.models.items():
            # Add foreign key relationships
            for fk_field, related_model in model.foreign_keys.items():
                if related_model != 'Unknown':
                    relationships[model_name].append(related_model)
            
            # Add many-to-many relationships
            for m2m_field, related_model in model.many_to_many.items():
                if related_model != 'Unknown':
                    relationships[model_name].append(related_model)
                    # Bidirectional relationship
                    relationships[related_model].append(model_name)
        
        return dict(relationships)
    
    def _generate_project_recommendations(self, models: Dict[str, DjangoModel], 
                                        code_issues: List[CodeIssue], 
                                        relationships: Dict[str, List[str]]) -> List[str]:
        """Generate project-wide optimization recommendations."""
        recommendations = []
        
        # Index recommendations
        index_suggestions = self.suggest_indexes()
        if index_suggestions:
            recommendations.append(f"📊 **Database Indexes**: Add {sum(len(indexes) for indexes in index_suggestions.values())} recommended indexes")
        
        # Code issue recommendations
        critical_issues = [issue for issue in code_issues if issue.severity == 'critical']
        high_issues = [issue for issue in code_issues if issue.severity == 'high']
        
        if critical_issues:
            recommendations.append(f"🚨 **Critical Issues**: Fix {len(critical_issues)} critical performance issues")
        
        if high_issues:
            recommendations.append(f"⚠️ **High Priority**: Address {len(high_issues)} high-priority optimizations")
        
        # Model recommendations
        models_without_indexes = [m for m in models.values() if not m.indexes and (m.foreign_keys or m.many_to_many)]
        if models_without_indexes:
            recommendations.append(f"🏷️ **Model Optimization**: {len(models_without_indexes)} models need index optimization")
        
        # Template recommendations
        template_issues = [issue for issue in code_issues if issue.issue_type.startswith('template')]
        if template_issues:
            recommendations.append(f"📝 **Template Optimization**: Fix {len(template_issues)} template-level N+1 patterns")
        
        return recommendations
    
    def _calculate_optimization_score(self, code_issues: List[CodeIssue], models: Dict[str, DjangoModel]) -> float:
        """Calculate overall optimization score (0-100)."""
        score = 100.0
        
        # Deduct points for code issues
        for issue in code_issues:
            if issue.severity == 'critical':
                score -= 15
            elif issue.severity == 'high':
                score -= 8
            elif issue.severity == 'medium':
                score -= 3
            elif issue.severity == 'low':
                score -= 1
        
        # Deduct points for models without indexes
        for model in models.values():
            if (model.foreign_keys or model.many_to_many) and not model.indexes:
                score -= 5
        
        # Ensure score doesn't go below 0
        return max(0.0, score)
    
    def _generate_project_summary(self, models: Dict[str, DjangoModel], 
                                code_issues: List[CodeIssue], 
                                relationships: Dict[str, List[str]], 
                                score: float) -> str:
        """Generate project analysis summary."""
        summary_lines = [
            f"Django Project Analysis Summary",
            f"===============================",
            f"Project: {self.project_root.name}",
            f"Optimization Score: {score:.1f}/100",
            f"",
            f"📊 **Project Statistics:**",
            f"- Django Apps: {len(self.apps)}",
            f"- Models: {len(models)}",
            f"- Code Issues Found: {len(code_issues)}",
            f"- Model Relationships: {sum(len(related) for related in relationships.values())}",
            f"",
        ]
        
        # Issue breakdown
        issue_counts = defaultdict(int)
        for issue in code_issues:
            issue_counts[issue.severity] += 1
        
        if issue_counts:
            summary_lines.extend([
                f"🔍 **Issues by Severity:**",
                f"- Critical: {issue_counts['critical']}",
                f"- High: {issue_counts['high']}",
                f"- Medium: {issue_counts['medium']}",
                f"- Low: {issue_counts['low']}",
                f"",
            ])
        
        # Model analysis
        models_with_relationships = len([m for m in models.values() if m.foreign_keys or m.many_to_many])
        models_with_indexes = len([m for m in models.values() if m.indexes])
        
        summary_lines.extend([
            f"🏗️ **Model Analysis:**",
            f"- Models with relationships: {models_with_relationships}/{len(models)}",
            f"- Models with custom indexes: {models_with_indexes}/{len(models)}",
            f"",
        ])
        
        # Performance recommendations
        if score < 70:
            summary_lines.append("🚨 **Action Required**: This project has significant optimization opportunities")
        elif score < 85:
            summary_lines.append("⚠️ **Improvements Needed**: Some optimization opportunities identified")
        else:
            summary_lines.append("✅ **Good Performance**: Project follows most ORM best practices")
        
        return "\n".join(summary_lines)
    
    def _get_app_name_for_model(self, model: DjangoModel) -> str:
        """Get the Django app name for a model."""
        model_path = Path(model.file_path)
        return model_path.parent.name
    
    def _generate_migration_code(self, model_name: str, indexes: List[List[str]]) -> str:
        """Generate Django migration code for indexes."""
        operations = []
        
        for fields in indexes:
            if len(fields) == 1:
                operations.append(f"        migrations.RunSQL('CREATE INDEX CONCURRENTLY idx_{model_name.lower()}_{fields[0]} ON {model_name.lower()} ({fields[0]});'),")
            else:
                field_names = '_'.join(fields)
                field_list = ', '.join(fields)
                operations.append(f"        migrations.RunSQL('CREATE INDEX CONCURRENTLY idx_{model_name.lower()}_{field_names} ON {model_name.lower()} ({field_list});'),")
        
        return f"""operations = [
{chr(10).join(operations)}
]"""


def analyze_django_project(project_path: str) -> ProjectAnalysisResult:
    """
    Analyze a Django project for ORM optimization opportunities.
    
    Args:
        project_path: Path to Django project root (containing manage.py)
    
    Returns:
        ProjectAnalysisResult with comprehensive analysis
    """
    analyzer = DjangoProjectAnalyzer(project_path)
    return analyzer.analyze_project()


def generate_optimization_report(project_path: str, output_file: Optional[str] = None) -> str:
    """
    Generate a comprehensive optimization report for a Django project.
    
    Args:
        project_path: Path to Django project root
        output_file: Optional file to save the report
    
    Returns:
        Report content as string
    """
    result = analyze_django_project(project_path)
    
    report_lines = [
        result.summary,
        "",
        "🎯 **Priority Recommendations:**",
        ""
    ]
    
    for i, recommendation in enumerate(result.recommendations[:5], 1):
        report_lines.append(f"{i}. {recommendation}")
    
    report_lines.extend([
        "",
        "🔧 **Detailed Code Issues:**",
        "========================",
        ""
    ])
    
    # Group issues by file
    issues_by_file = defaultdict(list)
    for issue in result.code_issues:
        issues_by_file[issue.file_path].append(issue)
    
    for file_path, file_issues in list(issues_by_file.items())[:10]:  # Show top 10 files
        report_lines.extend([
            f"📁 {Path(file_path).name}",
            "-" * 40
        ])
        
        for issue in file_issues[:5]:  # Show top 5 issues per file
            report_lines.extend([
                f"Line {issue.line_number}: {issue.description}",
                f"Severity: {issue.severity.upper()}",
                f"Suggestion: {issue.suggestion.split(chr(10))[0]}...",  # First line only
                ""
            ])
    
    report_content = "\n".join(report_lines)
    
    if output_file:
        with open(output_file, 'w', encoding='utf-8') as f:
            f.write(report_content)
        print(f"📄 Report saved to {output_file}")
    
    return report_content