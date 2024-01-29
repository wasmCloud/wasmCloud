import * as vscode from 'vscode';
import {ProviderNode} from './ProviderNode';
import {WadmProvider} from '@wasmcloud/lattice-client-core';
import {sortById} from '@/lib/sort';
import {LatticeNodeWithChildren} from './types';

export class ProvidersNode extends LatticeNodeWithChildren {
  #providers: WadmProvider[];

  constructor(providers: Record<string, WadmProvider>) {
    const count = Object.keys(providers).length;
    super(
      'Providers',
      count > 0 ? vscode.TreeItemCollapsibleState.Collapsed : vscode.TreeItemCollapsibleState.None,
    );
    this.#providers = sortById(providers);
    this.description = count.toString();
  }

  async getChildren(): Promise<ProviderNode[]> {
    return this.#providers.map((provider) => new ProviderNode(provider));
  }
}
