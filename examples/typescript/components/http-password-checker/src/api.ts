/**
 * @file Helpers and utility types for dealing with requests
 */

import * as v from "valibot";
import { passwordStrength } from "check-password-strength";

/** START wasi generated imports */
// NOTE import paths are aliased in tsconfig.json
import {
  IncomingRequest,
  ResponseOutparam,
  OutgoingBody,
  OutgoingResponse,
  Fields,
  InputStream,
} from "wasi:http/types@0.2.0";
import * as wasmcloudSecretsReveal from "wasmcloud:secrets/reveal@0.1.0-draft";
import * as wasmcloudSecretsStore from "wasmcloud:secrets/store@0.1.0-draft";
/**  END wasi generated imports */

import { PasswordStrength, PASSWORD_CHECK_RULES } from "./passwords.js";
import { readInputStream, sendResponseJSON } from "./wasi.js";
import { ResponseStatus, Response } from "./http.js";

/**
 * Represents an API request for checking a password
 *
 * @class
 */
export class PasswordCheckRequest {
  // For use when checking a value that exists in the secret store
  // but is pointed to by the API request
  public secret?: {
    key: string;
    field?: string;
  };

  // Used when checking a value directly submitted in the API request
  public value?: string;

  /** Schema that can be used to parse an object */
  static schema() {
    return v.object({
      secret: v.optional(
        v.object({
          key: v.string(),
          field: v.optional(v.string()),
        })
      ),
      value: v.optional(v.string()),
    });
  }

  /** Parse a PasswordCheckRequest from a wasi:http `IncomingRequest` */
  static async fromRequest(
    req: IncomingRequest
  ): Promise<PasswordCheckRequest> {
    try {
      let bytes = readInputStream(req.consume().stream());
      let obj = JSON.parse(new TextDecoder("utf8").decode(bytes));
      return v.parse(PasswordCheckRequest.schema(), obj);
    } catch (err) {
      throw new Error(
        `failed to parse incoming data as a PasswordCheckRequest: ${err?.toString()}`
      );
    }
  }
}

/** Create a PasswordStrength enum value from a ID provided by `check-password-strength` */
function passwordStrengthFromID(id: number): PasswordStrength {
  switch (id) {
    case 0:
      return PasswordStrength.VeryWeak;
    case 1:
      return PasswordStrength.Weak;
    case 2:
      return PasswordStrength.Medium;
    case 3:
      return PasswordStrength.Strong;
    default:
      throw new Error(`invalid check-password-strength ID [${id}]`);
  }
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
 * @param {PasswordCheckRequest} cr - The Check request to complete
 * @returns {Promise<CheckResult>} A promise that resolves to the HTTP response with the check result
 */
export async function handlePasswordCheck(
  cr: PasswordCheckRequest
): Promise<Response<CheckResult>> {
  if (!cr.value && !cr.secret) {
    throw new Error("value or secret must be provided");
  }

  // Determine the value
  let value: string;
  if (cr.value) {
    // For directly usable values, we can just take the value
    value = cr.value;
  } else {
    if (!cr.secret) {
      throw new Error("Unexpectedly missing request secret");
    }

    // Retrieve the secret
    let secret: wasmcloudSecretsStore.Secret;
    try {
      secret = wasmcloudSecretsStore.WasmcloudSecretsStore.get(cr.secret.key);
    } catch (err) {
      throw new Error("failed to get secret");
    }

    // Reveal the secret
    try {
      const revealed =
        wasmcloudSecretsReveal.WasmcloudSecretsReveal.reveal(secret);
      if (revealed.tag != "string") {
        throw new Error("unexpected tag, secret should be a string");
      }
      value = revealed.val;
    } catch (err) {
      throw new Error(`failed to get secret: ${err?.toString()}`);
    }
  }

  const {
    id,
    value: strength,
    contains,
    length,
  } = passwordStrength(value, PASSWORD_CHECK_RULES);

  return Response.ok({
    strength,
    length,
    contains,
  });
}
