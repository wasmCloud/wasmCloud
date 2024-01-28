from hello import exports
from hello.types import Ok
from hello.imports.types import (
    IncomingRequest, ResponseOutparam,
    OutgoingResponse, Fields, OutgoingBody
)

class IncomingHandler(exports.IncomingHandler):
    def handle(self, _: IncomingRequest, response_out: ResponseOutparam):
        # Construct the HTTP response to send back 
        outgoingResponse = OutgoingResponse(Fields.from_list([]))
        # Set the status code to OK
        outgoingResponse.set_status_code(200)
        outgoingBody = outgoingResponse.body()
        # Write our Hello World message to the response body
        outgoingBody.write().blocking_write_and_flush(bytes("Hello from Python!\n", "utf-8"))
        OutgoingBody.finish(outgoingBody, None)
        # Set and send the HTTP response
        ResponseOutparam.set(response_out, Ok(outgoingResponse))