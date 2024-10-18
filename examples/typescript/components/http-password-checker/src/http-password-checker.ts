import {
  IncomingRequest,
  ResponseOutparam,
  OutgoingBody,
  OutgoingResponse,
  Fields,
  MethodGet,
} from "wasi:http/types@0.2.0";

class CheckRequest {
  // For use when checking a value that exists in the secret store
  // but is pointed to by the API request
  public secret?: {
    name?: string;
    key?: string;
    field?: string;
  };

  // Used when checking a value directly submitted in the API request
  public value?: string;

  /** Parse a CheckRequest from a wasi:http `IncomingRequest` */
  static async fromRequest(req: IncomingRequest): Promise<CheckRequest> {
    throw new Error("NOT IMPLEMENTED");
  }
}

/** Status of a HTTP response (see `Response<T>`) */
enum ResponseStatus {
  Success = "success",
  Error = "error",
}

/** Error code that encompasses all the errors the API can emit */
enum ErrorCode {
  UnexpectedError = "unexpected-error",
}

/** Generic envelope container for responses */
class Response<T> {
  public status: ResponseStatus = ResponseStatus.Success;
  public data?: T;
  public error?: {
    code: ErrorCode;
    msg?: string;
  };

  static ok<T>(data: T): Response<T> {
    return {
      status: ResponseStatus.Success,
      data,
    };
  }

  static error<T>(code: ErrorCode, msg?: string): Response<T> {
    return {
      status: ResponseStatus.Error,
      error: {
        code,
        msg,
      },
    };
  }
}

enum PasswordStrength {
  VeryWeak = "very-weak",
  Weak = "weak",
  Medium = "medium",
  Strong = "strong",
}

/** API response for a check result */
interface CheckResult {
  /** Strength of the password */
  strength: PasswordStrength;
  /** Length of the password */
  length: number;
  /** The types of characters the password contains (e.g. 'lowercase', 'uppercase', 'symbol', etc) */
  contains: string[];
}

/**
 * Perform a check for a given request
 *
 * This function can check a password whether it's been provided or is a secret.
 *
 * @param {CheckRequest} cr - The Check request to complete
 * @returns {Promise<CheckResult>} A promise that resolves to the HTTP response with the check result
 */
async function handleSecretCheck(
  cr: CheckRequest
): Promise<Response<CheckResult>> {
  throw new Error("NOT IMPLEMENTED");
}

/**
 * Implementation of the `wasi:http/incoming-handler` interface that is exported by the component
 */
export const incomingHandler = {
  // Implementation of wasi-http incoming-handler
  async handle(req: IncomingRequest, resp: ResponseOutparam) {
    // Only allow GET requests
    if (req.method().tag != "get") {
      await sendResponseJSON(resp, 400, {
        status: "error",
        message: "invalid request, must be GET",
      });
      return;
    }

    // Parse out the request path
    let [path, maybeQuery] = (req.pathWithQuery() ?? "").split("?");

    // Handle request
    switch (path) {
      case "/api/v1/check":
        // Parse the check request from the body
        let cr: CheckRequest;
        try {
          cr = await CheckRequest.fromRequest(req);
        } catch (err) {
          await sendResponseJSON(resp, 400, {
            status: "error",
            message: "invalid request body",
          });
          return;
        }

        // Perform the check
        let checkResponse = await handleSecretCheck(cr);

        // Send the HTTP response
        await sendResponseJSON(resp, 200, checkResponse);
      default:
        await sendResponseJSON(resp, 400, {
          status: "error",
          message: "invalid API request",
        });
    }
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
 * Helper for sending JSON HTTP responses
 *
 * @param {unknown} body - A JSON  of the request to be sent (must be less than 4096 bytes)
 * @param {number} httpStatus - HTTP status code
 */
async function sendResponseJSON(
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
