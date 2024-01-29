import {WadmProvider} from '@wasmcloud/lattice-client-core';
import * as vscode from 'vscode';
import {SimpleHostsNode} from '@/lattice/SimpleHostsNode';
import {buildTreeItem} from '@/lib/node-helpers';
import {LatticeNodeWithData} from './types';

export class ProviderNode extends LatticeNodeWithData {
  #provider: WadmProvider;

  constructor(provider: WadmProvider) {
    super(provider.name, vscode.TreeItemCollapsibleState.Collapsed);
    this.description = provider.contract_id;
    this.#provider = provider;
  }

  async getData(): Promise<vscode.TreeItem[]> {
    return [
      buildTreeItem(this.#provider, {
        label: 'Contract',
        contextValue: 'copyText',
        description: (provider: WadmProvider) => provider.contract_id,
      }),
      buildTreeItem(this.#provider, {
        label: 'Reference',
        contextValue: 'copyText',
        description: (provider: WadmProvider) => provider.reference,
      }),
      buildTreeItem(this.#provider, {
        label: 'Link Name',
        contextValue: 'copyText',
        description: (provider: WadmProvider) => provider.link_name,
      }),
      buildTreeItem(this.#provider, {
        label: 'ID',
        contextValue: 'copyText',
        description: (provider: WadmProvider) => provider.id,
      }),
      new SimpleHostsNode(Object.keys(this.#provider.hosts)),
    ];
  }
}
