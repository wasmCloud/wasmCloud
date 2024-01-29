import {sortById} from '@/lib/sort';
import {WadmHost} from '@wasmcloud/lattice-client-core';
import * as vscode from 'vscode';
import {HostNode} from './HostNode';
import {LatticeNodeWithChildren} from './types';

export class HostsNode extends LatticeNodeWithChildren {
  #hosts: WadmHost[];

  constructor(hosts: Record<string, WadmHost>) {
    const count = Object.keys(hosts).length;
    super(
      'Hosts',
      count > 0 ? vscode.TreeItemCollapsibleState.Collapsed : vscode.TreeItemCollapsibleState.None,
    );
    this.#hosts = sortById(hosts);
    this.description = count.toString();
  }

  async getChildren(): Promise<vscode.TreeItem[]> {
    return this.#hosts.map((host) => new HostNode(host));
  }
}
