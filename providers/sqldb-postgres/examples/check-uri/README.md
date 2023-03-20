This is a utility program to check the validity of the "uri" that goes into the linkdef file.

It does NOT use any part of the sqldb provider.

Usage:
  check-uri URI [ QUERY ]

The first parameter after the command name is the uri, as in
   "postgresql://user:pass@localhost/dbname"

The next parameter(s) can be a query to execute using the connection uri.


