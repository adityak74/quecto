import os
DEBUG = True
DATABASE_URL = os.getenv('DB_URL', 'postgres://user:pass@localhost:5432/mydb')
