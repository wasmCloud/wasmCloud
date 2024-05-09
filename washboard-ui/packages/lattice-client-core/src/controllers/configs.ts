import {WasmCloudConfig, type ControlResponse} from '../types';
import {BaseController} from './base-controller';

type ConfigListRequest = {
  expand?: boolean;
};

type GetResponse = {
  [key: string]: string;
};

class ConfigsController extends BaseController {
  /**
   * Get the current lattice configuration
   * @param options.expand whether to expand the config details. Note that this can be a slow operation
   * @returns the current lattice configuration
   */
  async list(): Promise<ControlResponse<string[]>>;
  async list(options: ConfigListRequest & {expand: false}): Promise<ControlResponse<string[]>>;
  async list(
    options: ConfigListRequest & {expand: true},
  ): Promise<ControlResponse<WasmCloudConfig[]>>;
  async list(
    options: ConfigListRequest = {},
  ): Promise<ControlResponse<string[] | WasmCloudConfig[]>> {
    // TODO: this should be a NATS request once the control API is updated but we can just get the keys from the bucket
    const configKeys = await this.connection.getBucketKeys(`CONFIGDATA_${this.config.latticeId}`);

    if (!options?.expand) {
      return {
        success: true,
        message: 'Config keys retrieved successfully',
        response: configKeys,
      } satisfies ControlResponse<string[]>;
    }

    try {
      const configItems = await Promise.all(
        configKeys.map(async (key) => {
          const result = await this.get(key);
          if (!result.response) throw new Error(result.message);
          return {
            key,
            entries: result.response,
          };
        }),
      );

      return {
        success: true,
        message: 'Config items retrieved successfully',
        response: configItems,
      } satisfies ControlResponse<WasmCloudConfig[]>;
    } catch {
      return {
        success: false,
        message: 'Failed to retrieve config items',
        response: [],
      } satisfies ControlResponse<WasmCloudConfig[]>;
    }
  }

  /**
   * Set the lattice configuration
   * @param config the configuration to set
   * @returns A object of key-value pairs representing the contents of the config item wrapped in a
   * ControlResponse. If the config item does not exist, the `response` key will not be present.
   * Note that the `success` key will always be `true` if the request was successful.
   */
  async get(configKey: string) {
    return this.connection.request<ControlResponse<GetResponse>>(
      `${this.config.ctlTopic}.config.get.${configKey}`,
    );
  }

  /**
   * Puts a named config, replacing any data that is already present
   * @param name config name. Must be valid NATS subject strings and not contain any `.` or `>` characters
   * @param config the keys/values to put into the config
   * @returns request status
   */
  async put(name: string, config: Record<string, string>) {
    return this.connection.request<ControlResponse>(
      `${this.config.ctlTopic}.config.put.${name}`,
      JSON.stringify(config),
    );
  }

  /**
   * Delete the named config item
   * @param name Config name. Must be valid NATS subject strings and not contain any `.` or `>` characters.
   * @returns request status
   */
  async delete(name: string) {
    return this.connection.request<ControlResponse>(`${this.config.ctlTopic}.config.del.${name}`);
  }
}

export {ConfigsController};
