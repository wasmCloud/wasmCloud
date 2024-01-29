import * as vscode from 'vscode';
import {LatticeClient} from '@wasmcloud/lattice-client-core';
import {getLatticeClient} from '@/lattice/client';
import {ProvidersNode} from '@/lattice/ProvidersNode';
import {
  LatticeExplorerNode,
  LatticeExplorerRootNode,
  LatticeNodeWithChildren,
  LatticeNodeWithData,
} from '@/lattice/types';
import {HostsNode} from '@/lattice/HostsNode';
import {ActorsNode} from '@/lattice/ActorsNode';
import {LinksNode} from '@/lattice/LinksNode';

export class LatticeDataProvider implements vscode.TreeDataProvider<{}> {
  #client: Promise<LatticeClient | null>;

  private _onDidChangeTreeData: vscode.EventEmitter<LatticeExplorerNode | undefined | void> =
    new vscode.EventEmitter<LatticeExplorerNode | undefined | void>();
  readonly onDidChangeTreeData: vscode.Event<LatticeExplorerNode | undefined | void> =
    this._onDidChangeTreeData.event;

  refresh(): void {
    this._onDidChangeTreeData.fire();
  }

  constructor() {
    this.#client = getLatticeClient();
    this.#subscribeToLatticeCache();
  }

  getChildren(
    element?: LatticeNodeWithChildren | LatticeNodeWithData | undefined,
  ): vscode.ProviderResult<{}[]> {
    if (!element) {
      return this.#getRootNodes();
    }

    if (element instanceof LatticeNodeWithChildren) {
      return element.getChildren();
    }

    if (element instanceof LatticeNodeWithData) {
      return element.getData();
    }

    return Promise.resolve([]);
  }

  getTreeItem(element: {}): vscode.TreeItem | Thenable<vscode.TreeItem> {
    return element;
  }

  async #getRootNodes(): Promise<LatticeExplorerRootNode[]> {
    const client = await this.#client;
    if (!client) {
      return [];
    }

    if (!client.isConnected) {
      return [];
    }

    const cache = client.latticeCache$.value;
    if (!cache) {
      return [];
    }

    return [
      cache.hosts ? new HostsNode(cache.hosts) : null,
      cache.actors ? new ActorsNode(cache.actors) : null,
      cache.providers ? new ProvidersNode(cache.providers) : null,
      cache.links ? new LinksNode(cache.links) : null,
    ].filter((x): x is LatticeExplorerRootNode => x !== null);
  }

  async #subscribeToLatticeCache() {
    const client = await this.#client;
    
    if (!client) {
      return;
    }

    client.getLatticeCache$().subscribe(() => {
      this._onDidChangeTreeData.fire();
    });
  }
}
