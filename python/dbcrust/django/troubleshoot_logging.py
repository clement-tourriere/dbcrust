#!/usr/bin/env python3
"""
Troubleshooting guide for DBCrust Django middleware logging issues.

This file provides several solutions to try if you can see the headers but not the logs.
"""

# ===================================================================
# SOLUTION 1: Correct Django Logging Configuration
# ===================================================================

WORKING_LOGGING_CONFIG = {
    "version": 1,
    "disable_existing_loggers": False,
    "formatters": {
        "verbose": {
            "format": "{levelname} {asctime} {name}: {message}",
            "style": "{",
        },
    },
    "handlers": {
        "console": {
            "class": "logging.StreamHandler",
            "formatter": "verbose",
            "stream": "ext://sys.stdout",  # Explicit stdout
        },
        "file": {
            "class": "logging.FileHandler",
            "filename": "dbcrust_performance.log",
            "formatter": "verbose",
        },
    },
    "loggers": {
        "dbcrust.performance": {
            "handlers": ["console", "file"],
            "level": "DEBUG",
            "propagate": False,  # IMPORTANT: False prevents Django from filtering
        },
    },
}

# ===================================================================
# SOLUTION 2: Force Debug Mode Configuration
# ===================================================================

DEBUG_DBCRUST_CONFIG = {
    "ENABLED": True,
    "QUERY_THRESHOLD": 1,  # Very low threshold to catch everything
    "TIME_THRESHOLD": 1,  # Very low threshold
    "LOG_ALL_REQUESTS": True,
    "INCLUDE_HEADERS": True,
    "DEBUG_LOGGING": True,  # This prints to stderr as backup
}


# ===================================================================
# SOLUTION 3: Test Script
# ===================================================================

def test_logging_setup():
    """Test if your logging setup is working."""
    import logging
    import sys

    # Configure logging like your Django app
    logging.basicConfig(
        level=logging.DEBUG,
        format='%(levelname)s:%(name)s: %(message)s',
        stream=sys.stdout
    )

    # Test the specific logger
    logger = logging.getLogger('dbcrust.performance')
    logger.setLevel(logging.DEBUG)

    print("=" * 50)
    print("TESTING DBCRUST LOGGING")
    print("=" * 50)

    logger.debug("🔧 DEBUG: This should appear if DEBUG level works")
    logger.info("ℹ️  INFO: This should appear if INFO level works")
    logger.warning("⚠️  WARNING: This should appear if WARNING level works")
    logger.error("❌ ERROR: This should appear if ERROR level works")

    print("\n" + "=" * 50)
    print("If you see the emoji messages above, logging works!")
    print("If not, there's a Django logging configuration conflict.")
    print("=" * 50)


# ===================================================================
# SOLUTION 4: Alternative Settings.py Configuration
# ===================================================================

ALTERNATIVE_SETTINGS = """
# Add this to your settings.py

# Option A: Simple configuration (try this first)
LOGGING = {
    'version': 1,
    'disable_existing_loggers': False,
    'handlers': {
        'console': {
            'class': 'logging.StreamHandler',
        },
    },
    'root': {
        'handlers': ['console'],
        'level': 'INFO',
    },
    'loggers': {
        'dbcrust.performance': {
            'handlers': ['console'],
            'level': 'DEBUG',
            'propagate': False,  # This is KEY!
        },
    },
}

# Option B: If Option A doesn't work, try this more explicit version
import logging
import sys

LOGGING = {
    'version': 1,
    'disable_existing_loggers': False,
    'formatters': {
        'detailed': {
            'format': '%(asctime)s [%(levelname)s] %(name)s: %(message)s'
        }
    },
    'handlers': {
        'stdout': {
            'class': 'logging.StreamHandler',
            'stream': sys.stdout,
            'formatter': 'detailed',
        }
    },
    'loggers': {
        'dbcrust.performance': {
            'level': 'INFO',
            'handlers': ['stdout'],
            'propagate': False,
        }
    }
}

# DBCrust Configuration with debug mode
DBCRUST_PERFORMANCE_ANALYSIS = {
    "ENABLED": True,
    "QUERY_THRESHOLD": 1,     # Very low to catch everything
    "TIME_THRESHOLD": 1,      # Very low to catch everything  
    "LOG_ALL_REQUESTS": True, # Force logging all requests
    "DEBUG_LOGGING": True,    # Also print to stderr as backup
}
"""

# ===================================================================
# SOLUTION 5: Manual Test in Django Shell
# ===================================================================

MANUAL_TEST = """
# Run this in Django shell (python manage.py shell)

import logging
logger = logging.getLogger('dbcrust.performance')

# Test basic logging
logger.info("Test message from Django shell")
logger.warning("Test warning from Django shell") 
logger.error("Test error from Django shell")

# Check logger configuration  
print(f"Logger name: {logger.name}")
print(f"Logger level: {logger.level}")
print(f"Logger handlers: {logger.handlers}")
print(f"Logger propagate: {logger.propagate}")

# Check if parent loggers are interfering
parent = logger.parent
while parent:
    print(f"Parent logger: {parent.name}, level: {parent.level}, handlers: {parent.handlers}")
    parent = parent.parent
"""


def main():
    """Main troubleshooting function."""
    print("🔍 DBCrust Django Middleware Logging Troubleshooter")
    print("=" * 60)

    print("\n📋 PROBLEM: You see headers but no console logs")
    print("Headers show: x-dbcrust-issues-total: 7")
    print("But no performance logs appear in Django console")

    print("\n🔧 SOLUTIONS TO TRY (in order):")

    print("\n1. 📝 UPDATE YOUR LOGGING CONFIG:")
    print("   Replace your LOGGING config with this:")
    print("   " + "=" * 50)
    import pprint
    pprint.pprint(WORKING_LOGGING_CONFIG, width=80, depth=4)

    print("\n2. 🎯 UPDATE YOUR DBCRUST CONFIG:")
    print("   Use these more aggressive settings:")
    print("   " + "=" * 50)
    pprint.pprint(DEBUG_DBCRUST_CONFIG, width=80, depth=2)

    print("\n3. 🧪 TEST LOGGING DIRECTLY:")
    print("   Run this test function:")
    test_logging_setup()

    print("\n4. 🔍 DJANGO SHELL TEST:")
    print("   Run this in Django shell (python manage.py shell):")
    print(MANUAL_TEST)

    print("\n5. 📄 ALTERNATIVE SETTINGS:")
    print(ALTERNATIVE_SETTINGS)

    print("\n✅ QUICK FIX:")
    print("   Add DEBUG_LOGGING: True to your DBCRUST config")
    print("   This will print to stderr even if Django logging is broken")


if __name__ == "__main__":
    main()
