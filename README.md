# waSCC Logging Provider
This library is a _native capability provider_ for the `wascc:logging` capability. Only actors signed with tokens containing this capability privilege will be allowed to use it.  It allows actors to use normal `log` macros to write logs from within the actor.

It should be compiled as a native linux (`.so`) binary and made available to the **waSCC** host runtime as a plugin. 

