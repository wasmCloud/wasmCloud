import {type ControlResponse} from '../types';
import {BaseController} from './base-controller';

type StartProviderBody = {
  /** The ID of the host to start the provider on */
  host_id: string;
  /** The OCI reference of the provider to start */
  provider_ref: string;
  /** The name of the link to use for this provider. If not specified, "default" is used */
  provider_id: string;
  /** any annotations for this provider */
  annotations: Record<string, string>;
  /** A list of named configs to use for this provider. It is not required to specify a config. */
  config?: string[];
};

type StopProviderBody = {
  /** The ID of the provider to stop */
  provider_id: string;
  /** The ID of the host to stop the provider on */
  host_id: string;
};

class ProvidersController extends BaseController {
  /**
   * Issues a command to a host to start a provider with a given OCI reference using the
   * specified link name (or "default" if none is specified). The target wasmCloud host will
   * acknowledge the receipt of this command _before_ downloading the provider's bytes from the
   * OCI registry, indicating either a validation failure or success. If a client needs
   * deterministic guarantees that the provider has completed its startup process, such a client
   * needs to monitor the control event stream for the appropriate event.
   *
   * @param body the body of the start request
   * @returns
   */
  async start(body: StartProviderBody) {
    return this.connection.request<ControlResponse>(
      `${this.config.ctlTopic}.provider.start`,
      JSON.stringify(body),
    );
  }

  /**
   * Issues a command to a host to stop a provider for the given OCI reference, link name, and
   * contract ID. The target wasmCloud host will acknowledge the receipt of this command, and
   * _will not_ supply a discrete confirmation that a provider has terminated. For that kind of
   * information, the client must also monitor the control event stream
   * @param body the body of the stop request
   * @returns
   */
  async stop(body: StopProviderBody) {
    return this.connection.request<ControlResponse>(
      `${this.config.ctlTopic}.provider.stop.${body.host_id}`,
      JSON.stringify(body),
    );
  }
}

export {ProvidersController};
