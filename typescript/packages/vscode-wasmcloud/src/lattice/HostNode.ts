import {buildTreeItem} from '@/lib/node-helpers';
import {WadmHost} from '@wasmcloud/lattice-client-core';
import * as vscode from 'vscode';
import {LatticeNodeWithData} from './types';

export class HostNode extends LatticeNodeWithData {
  #host: WadmHost;

  constructor(host: WadmHost) {
    super(host.friendly_name, vscode.TreeItemCollapsibleState.Collapsed);
    this.#host = host;
    this.description = host.version;
  }

  async getData(): Promise<vscode.TreeItem[]> {
    return [
      buildTreeItem(this.#host, {
        label: 'Name',
        contextValue: 'copyText',
        description: (host: WadmHost) => host.friendly_name,
      }),
      buildTreeItem(this.#host, {
        label: 'ID',
        contextValue: 'copyText',
        description: (host: WadmHost) => host.id,
      }),
      buildTreeItem(this.#host, {
        label: 'Version',
        contextValue: 'copyText',
        description: (host: WadmHost) => host.version,
      }),
      buildTreeItem(this.#host, {
        label: 'Last Seen',
        contextValue: 'copyText',
        description: (host: WadmHost) => new Date(host.last_seen).toLocaleString(),
      }),
      // actors
      // providers
      // labels
      // annotations
    ];
  }
}
