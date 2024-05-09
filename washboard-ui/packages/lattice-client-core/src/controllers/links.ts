import {WasmCloudLink, type ControlResponse} from '../types';
import {BaseController} from './base-controller';

type PutLinkRequest = {
  /** Source identifier for the link */
  source_id: string;
  /** Target for the link, which can be a unique identifier */
  target: string;
  /** Name of the link. Not providing this is equivalent to specifying "default" */
  name?: string;
  /** WIT namespace of the link operation, e.g. `wasi` in `wasi:keyvalue/readwrite.get` */
  wit_namespace: string;
  /** WIT package of the link operation, e.g. `keyvalue` in `wasi:keyvalue/readwrite.get` */
  wit_package: string;
  /** WIT Interfaces to be used for the link, e.g. `readwrite`, `atomic`, etc. */
  interfaces: string[];
  /** List of named configurations to provide to the source upon request */
  source_config?: string[];
  /** List of named configurations to provide to the target upon request */
  target_config?: string[];
};

type DeleteLinkRequest = {
  /** source identifier for the link */
  source_id: string;
  /** name of the link */
  link_name?: string;
  /** WIT namespace of the link operation, e.g. `wasi` in `wasi:keyvalue/readwrite.get` */
  wit_namespace: string;
  /** WIT package of the link operation, e.g. `keyvalue` in `wasi:keyvalue/readwrite.get` */
  wit_package: string;
};

class LinksController extends BaseController {
  /**
   * Get all links in the lattice
   * @returns all of the links in the lattice
   */
  async list() {
    return this.connection.request<ControlResponse<WasmCloudLink[]>>(
      `${this.config.ctlTopic}.link.get`,
    );
  }

  /**
   * Puts a link into the lattice. Returns an error if it was unable to put the link
   * @param link the link configuration
   * @returns
   */
  async put(link: PutLinkRequest) {
    return this.connection.request<ControlResponse>(
      `${this.config.ctlTopic}.link.put`,
      JSON.stringify(link),
    );
  }

  /**
   * Deletes a link from the lattice metadata keyvalue bucket. Returns an error if it was unable
   * to delete. This is an idempotent operation.
   * @param link the configuration of the link to delete
   * @returns
   */
  async delete(link: DeleteLinkRequest) {
    return this.connection.request<ControlResponse>(
      `${this.config.ctlTopic}.link.del`,
      JSON.stringify(link),
    );
  }
}

export {LinksController};
