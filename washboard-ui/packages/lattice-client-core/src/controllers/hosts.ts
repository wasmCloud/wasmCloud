import {BaseController} from '@/controllers/base-controller';
import {type ControlResponse, type WasmCloudHostRef, type WasmCloudHost} from '@/types';

type WadmStateBucketHost = {
  friendly_name: string;
  labels: Record<string, string>;
  components: Record<string, number>;
  providers: Array<{
    provider_id: string;
    provider_ref: string;
    annotations: Record<string, string>;
  }>;
  uptime_seconds: number;
  version: string;
  id: string;
  last_seen: string;
};

type HostListRequest = {
  expand?: boolean;
};

class HostsController extends BaseController {
  /**
   * List all hosts in the lattice. If the expand option is set to true, the host details will be loaded and returned.
   * If any of the hosts fail to load, an error will be returned in place of the host details.
   * @param options.expand whether to expand the host details
   * @returns hosts as a list of host refs, or a list of host details if expand is true
   */
  async list<Expand extends boolean>(
    options: HostListRequest & {expand?: Expand},
  ): Promise<
    Expand extends false ? ControlResponse<WasmCloudHostRef[]> : ControlResponse<WasmCloudHost[]>
  >;
  async list(
    options: HostListRequest = {},
  ): Promise<ControlResponse<WasmCloudHostRef[] | WasmCloudHost[]>> {
    const hostsFromWadm = await this.connection.getBucketEntry<Record<string, WadmStateBucketHost>>(
      'wadm_state',
      `host_${this.config.latticeId}`,
    );

    // convert the host data from the wadm state bucket into 'refs'
    const hostRefs: WasmCloudHostRef[] = Object.values(hostsFromWadm).map((host) => ({
      ...host,
      providers: host.providers.reduce((providers: Record<string, number>, {provider_id}) => {
        providers[provider_id] = (providers[provider_id] ?? 0) + 1;
        return providers;
      }, {}),
      lattice: this.config.latticeId,
    }));

    // if we don't want to expand the host details, return the refs
    if (!options?.expand) {
      return {
        success: true,
        message: 'Hosts retrieved successfully',
        response: hostRefs,
      } satisfies ControlResponse<WasmCloudHostRef[]>;
    }

    try {
      const hostDetails = await Promise.all(
        hostRefs.map(async ({id}) => {
          const hostDetail = await this.get(id);
          if (!hostDetail.success) {
            throw new Error('Failed to retrieve host details: ' + hostDetail.message);
          }
          return hostDetail.response;
        }),
      );

      return {
        success: true,
        message: 'Hosts retrieved successfully',
        response: hostDetails,
      } satisfies ControlResponse<WasmCloudHost[]>;
    } catch (error: unknown) {
      const message = error instanceof Error ? error.message : 'Failed to retrieve host details';
      return {
        success: false,
        message,
        response: [],
      } satisfies ControlResponse<Array<WasmCloudHost>>;
    }
  }

  /**
   * Get the inventory of a currently running host
   * @param hostId the ID of the host to get
   * @returns the host details
   */
  async get(hostId: string) {
    const result = await this.connection.request<ControlResponse<WasmCloudHost>>(
      `${this.config.ctlTopic}.host.get.${hostId}`,
    );

    return result;
  }

  /**
   * Issues a command to a specific host to perform a graceful termination. The target host will acknowledge receipt of
   * the command before it attempts a shutdown. To deterministically verify that the host is down, a client should
   * monitor for the "host stopped" event or passively detect the host down by way of a lack of heartbeat receipts
   * @param hostId the ID of the host to stop
   * @returns the response from the lattice
   */
  async stop(hostId: string) {
    const result = await this.connection.request<ControlResponse>(
      `${this.config.ctlTopic}.host.stop.${hostId}`,
      '{}', // needs empty payload: https://github.com/wasmCloud/wasmCloud/issues/2113
    );

    return result;
  }

  /**
   * Put a new (or update an existing) label on the given host. If the label already exists, it will be updated with the
   * new value.
   * @param hostId the ID of the host to put the label on
   * @param key the key of the label
   * @param value the value of the label
   * @returns the response from the lattice
   * @throws if the request fails
   */
  async putLabel(hostId: string, key: string, value: string) {
    const result = await this.connection.request<ControlResponse>(
      `${this.config.ctlTopic}.label.put.${hostId}`,
      JSON.stringify({key, value}),
    );

    return result;
  }

  /**
   * Delete a label from the given host
   * @param hostId the ID of the host to delete the label from
   * @param key the key of the label to delete
   * @returns the response from the lattice
   * @throws if the request fails
   */
  async deleteLabel(hostId: string, key: string) {
    const result = await this.connection.request<ControlResponse>(
      `${this.config.ctlTopic}.label.del.${hostId}`,
      JSON.stringify({key}),
    );

    return result;
  }
}

export {HostsController};
