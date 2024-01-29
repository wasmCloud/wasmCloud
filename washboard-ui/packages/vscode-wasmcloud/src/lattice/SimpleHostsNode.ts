import * as vscode from 'vscode';
import {getLatticeClient} from '@/lattice/client';
import {SimpleHostNode} from '@/lattice/SimpleHostNode';
import {LatticeNodeWithChildren} from './types';

export class SimpleHostsNode extends LatticeNodeWithChildren {
  #hostIds: string[];

  constructor(hosts: string[]) {
    const count = hosts.length;
    super(
      'Hosts',
      count > 0 ? vscode.TreeItemCollapsibleState.Collapsed : vscode.TreeItemCollapsibleState.None,
    );
    this.#hostIds = hosts;
    this.description = count.toString();
  }

  async getChildren(): Promise<vscode.TreeItem[]> {
    const client = await getLatticeClient();
    const hosts = client.latticeCache$.value?.hosts;
    return (
      this.#hostIds
        .sort((a, b) => a.localeCompare(b))
        .map((hostId) => {
          const host = hosts?.[hostId];
          return host ? new SimpleHostNode(host) : null;
        })
        .filter((x): x is SimpleHostNode => !!x) ?? []
    );
  }
}
