import {BaseController} from '@/controllers/base-controller';
import {type ControlResponse} from '@/types';

type ComponentScaleRequest = {
  /** the ID of the host to which the scale command should be sent */
  host_id: string;
  /** the OCI reference of the component to scale */
  component_ref: string;
  /** the ID of the component to scale */
  component_id: string;
  /** the maximum number of instances to scale to. Specifying 0 will stop the component */
  max_instances: number;
  /** a set of key-value pairs to associate with the component */
  annotations: Record<string, string>;
  /** a list of named configs to apply to the component */
  config: string[];
};

type ComponentUpdateRequest = {
  /** the ID of the host to which the update command should be sent */
  host_id: string;
  /** the ID of the component to update */
  component_id: string;
  /** the new OCI reference for the component */
  new_component_ref: string;
  /** a set of key-value pairs to associate with the component */
  annotations: Record<string, string>;
};

class ComponentController extends BaseController {
  /**
   * Sends a request to the given host to scale a given component. This returns an acknowledgement of _receipt_ of the
   * command, not a confirmation that the component scaled. An acknowledgement will either indicate some form of
   * validation failure, or, if no failure occurs, the receipt of the command. To avoid blocking consumers, wasmCloud
   * hosts will acknowledge the scale component command prior to fetching the component's OCI bytes. If you need
   * deterministic results as to whether the component completed its startup process, you will have to monitor the
   * appropriate event through `latticeClient.subscribe()`
   * @param body the request body
   * @returns the response from the scale request
   */
  async scale(body: ComponentScaleRequest) {
    const response = this.connection.request<ControlResponse>(
      `${this.config.ctlTopic}.component.scale.${body.host_id}`,
      JSON.stringify(body),
    );

    return response;
  }

  /**
   * Issue a command to a host instructing that it replace an existing component (indicated by its public key) with a
   * new component indicated by an OCI image reference. The host will acknowledge this request as soon as it verifies
   * that the target component is running. This acknowledgement occurs **before** the new bytes are downloaded.
   * Live-updating an component can take a long time and control clients cannot block waiting for a reply that could
   * come several seconds later. If you need to verify that the component has been updated, you will want to set up a
   * listener for the appropriate event through `latticeClient.subscribe()`
   * @param body.host_id the ID of the host to which the update command should be sent
   * @returns the response from the update request
   */
  async update(body: ComponentUpdateRequest) {
    const response = this.connection.request<ControlResponse>(
      `${this.config.ctlTopic}.component.update.${body.host_id}`,
      JSON.stringify(body),
    );

    return response;
  }
}

export {ComponentController};
