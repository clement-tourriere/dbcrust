"""
Django app configuration for DBCrust Django integration.

This configuration makes the dbcrust.django module a proper Django app
that can be added to INSTALLED_APPS to provide both ORM analysis and 
management command functionality.
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
        is ready to handle requests. We can use this for any initialization
        that needs to happen after Django is fully loaded.
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
            # For now, we'll just ensure the module loads properly
            
        except ImportError:
            # Django might not be available in all contexts
            pass
        except Exception:
            # Silently handle any other configuration issues
            # We don't want app loading to fail if there are minor issues
            pass