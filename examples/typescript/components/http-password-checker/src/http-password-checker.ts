import {
  IncomingRequest,
  ResponseOutparam,
  OutgoingBody,
  OutgoingResponse,
  Fields,
  MethodGet,
} from "wasi:http/types@0.2.0";

/**
 * Implementation of the `wasi:http/incoming-handler` interface that is exported by the component
 */
export const incomingHandler = {
  // Implementation of wasi-http incoming-handler
  async handle(req: IncomingRequest, resp: ResponseOutparam) {
    // Only allow GET requests
    if (req.method().tag != "get") {
      await sendResponseText(
        resp,
        400,
        JSON.stringify({
          status: "error",
          message: "invalid request, must be GET",
        })
      );
      return;
    }

    let pathWithQuery = req.pathWithQuery();

    // TODO: Check the URL, route to different handlers

    await sendResponseText(resp, 200, "This is a test!");
  },
};

/**
 * Helper for sending textual HTTP responses
 *
 * @param {string} body - Body of the request to be sent (must be less than 4096 bytes)
 * @param {number} httpStatus - HTTP status code
 */
async function sendResponseText(
  resp: ResponseOutparam,
  httpStatus: number,
  body: string
): Promise<void> {
  await sendResponse(
    resp,
    httpStatus,
    new Uint8Array(new TextEncoder().encode(body))
  );
}

/**
 * Helper function for writing a *small* response (known to be <4096 bytes)
 *
 * @param {ResponseOutparam} resp - Response (`wasi:http` response-outparam)
 * @param {Uint8Array} bytes - Bytes to be sent out as the response
 */
async function sendResponse(
  resp: ResponseOutparam,
  statusCode: number,
  bytes: Uint8Array
): Promise<void> {
  // Start building an outgoing response
  const outgoingResponse = new OutgoingResponse(new Fields());

  // Access the outgoing response body
  let outgoingBody = outgoingResponse.body();
  {
    // Create a stream for the response body
    let outputStream = outgoingBody.write();
    // Write the provided bytes to the output stream
    if (bytes.length <= 4096) {
      outputStream.blockingWriteAndFlush(bytes);
    } else {
      throw new Error("STREAMING RESPONSES NOT IMPLEMENTED YET");
    }
    // @ts-ignore: This is required in order to dispose the stream before we return
    outputStream[Symbol.dispose]();
  }

  // Set the status code for the response
  outgoingResponse.setStatusCode(statusCode);
  // Finish the response body
  OutgoingBody.finish(outgoingBody, undefined);
  // Set the created response
  ResponseOutparam.set(resp, { tag: "ok", val: outgoingResponse });
}
