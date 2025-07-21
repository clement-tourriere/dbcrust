from dbcrust import PyDatabase, PyConfig

class PostgresClient:
    """
    A Python wrapper for the Rust-based PostgreSQL client.
    """
    
    def __init__(self, host="localhost", port=5432, user="postgres", 
                 password="", dbname="postgres", use_config=True):
        """
        Create a new PostgreSQL client connection.
        
        Args:
            host (str): Database server hostname
            port (int): Database server port
            user (str): Database username
            password (str): Database password
            dbname (str): Database name
            use_config (bool): Whether to use saved configuration
        """
        if use_config:
            # Load configuration
            config = PyConfig.load()
            config_dict = config.as_dict()
            
            # Use explicit parameters if provided, otherwise use config
            host = host if host != "localhost" else config_dict.get("host", "localhost")
            port = port if port != 5432 else config_dict.get("port", 5432)
            user = user if user != "postgres" else config_dict.get("user", "postgres")
            dbname = dbname if dbname != "postgres" else config_dict.get("dbname", "postgres")
            
            # Only use password from config if one wasn't provided and config has save_password
            if not password and config_dict.get("save_password", False):
                password = config_dict.get("password", "")
        
        self.db = PyDatabase(host, port, user, password, dbname)
        
    def execute(self, query):
        """
        Execute an SQL query and return formatted results.
        
        Args:
            query (str): SQL query to execute
            
        Returns:
            str: Formatted query results
        """
        return self.db.execute_query(query)
    
    def list_databases(self):
        """
        List all available databases.
        
        Returns:
            str: Formatted list of databases
        """
        return self.db.list_databases()
    
    def list_tables(self):
        """
        List tables in the current database.
        
        Returns:
            str: Formatted list of tables
        """
        return self.db.list_tables()
    
    def connect_to_db(self, dbname):
        """
        Connect to a different database.
        
        Args:
            dbname (str): Name of the database to connect to
        """
        self.db.connect_to_db(dbname)
        
    @staticmethod
    def save_config(host="localhost", port=5432, user="postgres", 
                   dbname="postgres", password=None, save_password=False):
        """
        Save configuration to file.
        
        Args:
            host (str): Database server hostname
            port (int): Database server port
            user (str): Database username
            dbname (str): Database name
            password (str, optional): Database password
            save_password (bool): Whether to save the password
        """
        config = PyConfig()
        config_dict = {
            "host": host,
            "port": port,
            "user": user,
            "dbname": dbname,
            "save_password": save_password
        }
        
        if save_password and password:
            config_dict["password"] = password
            
        config.update_from_dict(config_dict)
        config.save()

    def run_command(self, command):
        """
        Execute a command using the run_command function.
        
        Args:
            command (str): SQL query or backslash command to execute
            
        Returns:
            str: Formatted command results
        """
        from dbcrust._internal import run_command
        
        # Build connection URL from current connection parameters
        # Note: This is a simplified approach - in production you'd want to handle
        # SSL parameters, connection options, etc.
        connection_url = f"postgresql://{self.db.user}@{self.db.host}:{self.db.port}/{self.db.current_database}"
        
        # Format arguments as a list for the Rust function
        args = ["dbcrust", connection_url, "-c", command]
        return run_command(args) 