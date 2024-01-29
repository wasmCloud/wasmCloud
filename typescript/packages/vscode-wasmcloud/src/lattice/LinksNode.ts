import {WadmLink} from '@wasmcloud/lattice-client-core';
import * as vscode from 'vscode';
import {LinkNode} from './LinkNode';
import {LatticeNodeWithChildren} from './types';

export class LinksNode extends LatticeNodeWithChildren {
  #links: WadmLink[];

  constructor(links: WadmLink[]) {
    const count = links.length;
    super(
      'Links',
      count > 0 ? vscode.TreeItemCollapsibleState.Collapsed : vscode.TreeItemCollapsibleState.None,
    );
    this.#links = links;
    this.description = count.toString();
  }

  async getChildren(): Promise<vscode.TreeItem[]> {
    return this.#links.map((link) => new LinkNode(link));
  }
}
