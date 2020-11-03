# Telnet Capability Provider

The telnet capability provider will start a new telnet (socket) server for each actor that binds to it, using the following configuration variables from the binding:

* `PORT` - the port number on which to start the server
* `MOTD` - A file name containing the "message of the day" or login banner for the telnet server
