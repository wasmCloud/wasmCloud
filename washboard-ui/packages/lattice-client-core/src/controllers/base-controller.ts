import {type LatticeClient} from '../lattice-client';

abstract class BaseController {
  #client: LatticeClient;

  /**
   * Returns the connection object for the client
   */
  protected get connection() {
    return this.#client.connection;
  }

  /**
   * Returns the configuration object for the client
   */
  protected get config() {
    return this.#client.instance.config;
  }

  constructor(client: LatticeClient) {
    this.#client = client;
  }
}

export {BaseController};
