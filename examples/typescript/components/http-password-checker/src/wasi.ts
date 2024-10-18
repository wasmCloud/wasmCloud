/**
 * @file Helpers for dealing with WASI
 */

/** START wasi generated imports (see tsconfig.json for aliases) */
import {
  ResponseOutparam,
  OutgoingBody,
  OutgoingResponse,
  Fields,
  InputStream,
} from "wasi:http/types@0.2.0";
/**  END wasi generated imports */

/** Amount to read from a wasi:io stream */
const WASI_IO_READ_MAX_BYTES = 4096n;

/**
 * Completely read an WASI input stream
 *
 * @param {InputStream} stream
 * @returns {Promise<Buffer>}
 */
export function readInputStream(stream: InputStream): Uint8Array {
  let buf = [];
  while (true) {
    try {
      const chunk = stream.blockingRead(WASI_IO_READ_MAX_BYTES);
      buf.push(...chunk);
      if (!chunk || chunk.length == 0) {
        return new Uint8Array(buf);
      }
    } catch (err) {
      // Rethrow errors that are *not* the stream being closed
      if ((err as any)?.payload?.tag === "closed") {
        return new Uint8Array(buf);
      } else {
        throw err;
      }
    }
  }
}

/**
 * Helper for sending textual HTTP responses
 *
 * @param {string} body - Body of the request to be sent (must be less than 4096 bytes)
 * @param {number} httpStatus - HTTP status code
 */
export async function sendResponseText(
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
 * Helper for sending JSON HTTP responses
 *
 * @param {unknown} body - A JSON  of the request to be sent (must be less than 4096 bytes)
 * @param {number} httpStatus - HTTP status code
 */
export async function sendResponseJSON(
  resp: ResponseOutparam,
  httpStatus: number,
  body: unknown
): Promise<void> {
  await sendResponseText(resp, httpStatus, JSON.stringify(body));
}

/**
 * Helper function for writing a *small* response (known to be <4096 bytes)
 *
 * @param {ResponseOutparam} resp - Response (`wasi:http` response-outparam)
 * @param {Uint8Array} bytes - Bytes to be sent out as the response
 */
export async function sendResponse(
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
