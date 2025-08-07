#!/usr/bin/env python3
"""
Debug script to test DBCrust Django middleware logging.
"""

import logging
import sys
import os

# Set up logging
logging.basicConfig(
    level=logging.DEBUG,
    format='%(levelname)s:%(name)s: %(message)s',
    stream=sys.stdout
)

# Test the dbcrust logger specifically
logger = logging.getLogger('dbcrust.performance')
logger.setLevel(logging.DEBUG)

print("Testing DBCrust logging...")
logger.debug("This is a DEBUG message")
logger.info("This is an INFO message")
logger.warning("This is a WARNING message")
logger.error("This is an ERROR message")

print("Testing if Django setup works...")
try:
    import django
    print(f"Django available: {django.VERSION}")
except ImportError:
    print("Django not available")

print("\nTesting middleware import...")
try:
    from dbcrust.django.middleware import PerformanceAnalysisMiddleware
    print("✅ Middleware import successful")
    
    # Test logger in middleware context
    from dbcrust.django.middleware import logger as middleware_logger
    print(f"Middleware logger: {middleware_logger.name}")
    print(f"Middleware logger level: {middleware_logger.level}")
    print(f"Middleware logger handlers: {middleware_logger.handlers}")
    
    middleware_logger.warning("TEST: Direct middleware logger warning")
    
except ImportError as e:
    print(f"❌ Middleware import failed: {e}")

print("\nIf you see the test messages above, logging is working!")
print("If not, there's a logging configuration issue.")