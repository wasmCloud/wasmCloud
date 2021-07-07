This actor links to both HttpServer and a Hello Provider

Upon receiving an http request with a query parameter "name",
the name is sent to the Hello server.

The hello server's response is returned back to the http client.

To build, `make` and `make run`

Test with:
- "curl localhost:8008"   (response should be 'Hello World')
- "curl localhost:8008/?name=Bob"   (response should be 'Hello Bob')

