#!/usr/bin/env python3
"""
Example script demonstrating the dbcrust PostgreSQL client.
"""
import getpass
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

if __name__ == "__main__":
    main() 