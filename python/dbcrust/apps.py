"""
Django app configuration for DBCrust.

This configuration makes the dbcrust package a proper Django app
that provides the DBCrust management command.
"""

from django.apps import AppConfig


class DbcrustConfig(AppConfig):
    """Django app configuration for DBCrust integration."""
    
    default_auto_field = 'django.db.models.BigAutoField'
    name = 'dbcrust'
    verbose_name = 'DBCrust Integration'
    
    def ready(self):
        """
        Called when Django starts up.
        
        This method is called once Django has loaded all models and
        is ready to handle requests.
        """
        # Import here to avoid circular imports
        try:
            from django.conf import settings
            
            # Check if Django is properly configured
            if not settings.configured:
                return
                
            # Validate that we have database configuration
            if not hasattr(settings, 'DATABASES') or not settings.DATABASES:
                return
                
            # Optional: Add any startup validation or initialization here
            
        except ImportError:
            # Django might not be available in all contexts
            pass
        except Exception:
            # Silently handle any other configuration issues
            # We don't want app loading to fail if there are minor issues
            pass