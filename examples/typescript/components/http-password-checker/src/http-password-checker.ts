/**
 * @file Implementation of the WIT interface defined by this component
 */

import * as v from "valibot";

/** START wasi generated imports (see tsconfig.json for aliases) */
import { IncomingRequest, ResponseOutparam } from "wasi:http/types@0.2.0";
/**  END wasi generated imports */

import { sendResponseJSON } from "./wasi.js";
import { ErrorCode, Response } from "./http.js";
import { PasswordCheckRequest, handlePasswordCheck } from "./api.js";

/**
 * Implementation of the `wasi:http/incoming-handler` interface that is exported by the component
 */
export const incomingHandler = {
  /** Implementation of the `handle` function inside the interface */
  async handle(req: IncomingRequest, resp: ResponseOutparam) {
    // Only allow GET requests
    if (req.method().tag != "post") {
      await sendResponseJSON(resp, 400, {
        status: "error",
        message: "invalid request, must be POST",
      });
      return;
    }

    // Parse out the request path
    let [path, maybeQuery] = (req.pathWithQuery() ?? "").split("?");

    // Handle request
    switch (path) {
      case "/api/v1/check":
        // Parse the check request from the body
        let cr: PasswordCheckRequest;
        try {
          cr = await PasswordCheckRequest.fromRequest(req);
        } catch (err) {
          const msg = v.isValiError(err)
            ? `[${err.issues.join(",")}]`
            : (err as any).payload ??
              err?.toString() ??
              "unexpected error while parsing request";
          return await sendResponseJSON(resp, 400, {
            status: "error",
            message: `invalid request body: ${msg}`,
          });
        }

        // Perform the check
        try {
          const result = await handlePasswordCheck(cr);
          return await sendResponseJSON(resp, 200, Response.ok(result));
        } catch (err) {
          return await sendResponseJSON(
            resp,
            500,
            Response.error(
              ErrorCode.UnexpectedError,
              `failed to check secret: ${err?.toString()}`
            )
          );
        }

        break;

      default:
        await sendResponseJSON(
          resp,
          400,
          Response.error(
            ErrorCode.InvalidRequest,
            "invalid API request (unknown endpoint)"
          )
        );
    }
  },
};
