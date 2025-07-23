#!/usr/bin/env python3
"""
Example script demonstrating the dbcrust PostgreSQL client and CLI integration.
"""
import getpass
import dbcrust
from dbcrust import PostgresClient

def main():
    # Get connection parameters
    host = input("Host [localhost]: ") or "localhost"
    port = int(input("Port [5432]: ") or "5432")
    user = input("Username [postgres]: ") or "postgres"
    password = getpass.getpass("Password: ")
    dbname = input("Database [postgres]: ") or "postgres"
    
    # Connect to the database
    client = PostgresClient(host, port, user, password, dbname)
    print(f"Connected to PostgreSQL on {host}:{port}")
    
    # Show available commands
    print("Available commands:")
    print("  \\q          - Quit the client")
    print("  \\l          - List all databases")
    print("  \\dt         - List tables in current database")
    print("  \\c <dbname> - Connect to a different database")
    print("  <SQL>       - Execute SQL query")
    
    # REPL loop
    while True:
        try:
            command = input("sql> ")
            if not command:
                continue
                
            if command == "\\q":
                break
            elif command == "\\l":
                print(client.list_databases())
            elif command == "\\dt":
                print(client.list_tables())
            elif command.startswith("\\c "):
                dbname = command[3:].strip()
                client.connect_to_db(dbname)
                print(f"Connected to database: {dbname}")
            else:
                # Execute SQL query
                result = client.execute(command)
                print(result)
                
        except KeyboardInterrupt:
            print("\nCancelled")
            continue
        except Exception as e:
            print(f"Error: {e}")
    
    print("Goodbye!")

def demonstrate_cli_integration():
    """Demonstrate different ways to use dbcrust from Python"""
    
    print("\n=== DBCrust Python API Demo ===\n")
    
    # Example 1: Direct command execution
    print("1. Direct Command Execution:")
    try:
        # This would work with a real database
        connection_url = "postgres://postgres@localhost/postgres"
        result = dbcrust.run_command(connection_url, "SELECT version()")
        print(f"Database version: {result}")
    except Exception as e:
        print(f"Connection failed (expected): {e}")
    
    print("\n2. Programmatic Execution with CLI Arguments:")
    try:
        # Execute with additional CLI flags - perfect for automation
        result = dbcrust.run_with_url(
            "postgres://postgres@localhost/postgres",
            ["--debug", "--no-banner", "-c", "\\dt"]
        )
        print(f"Tables: {result}")
    except Exception as e:
        print(f"Connection failed (expected): {e}")
    
    print("\n3. Clean programmatic calls for integration:")
    try:
        # No sys.argv conflicts - perfect for calling from other CLIs
        dbcrust.run_with_url("session://my_saved_session")
    except Exception as e:
        print(f"Session not found (expected): {e}")
    
    print("\n4. Interactive CLI (commented out - would launch interactive mode):")
    print("# dbcrust.run_cli('postgres://postgres@localhost/postgres')")
    
    print("\nDemo completed!")

if __name__ == "__main__":
    import sys
    if len(sys.argv) > 1 and sys.argv[1] == "demo":
        demonstrate_cli_integration()
    else:
        main() 