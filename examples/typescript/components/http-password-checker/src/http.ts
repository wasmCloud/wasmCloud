/**
 * @file Helpers and utility types for dealing with HTTP requests
 */

/** Status of a HTTP response (see `Response<T>`) */
export enum ResponseStatus {
  Success = "success",
  Error = "error",
}

/** Error code that encompasses all the errors the API can emit */
export enum ErrorCode {
  UnexpectedError = "unexpected-error",
  InvalidRequest = "invalid-request",
}

/** Generic envelope container for responses */
export class Response<T> {
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
