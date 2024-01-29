import {buildTreeItem} from '@/lib/node-helpers';
import {WadmLink} from '@wasmcloud/lattice-client-core';
import * as vscode from 'vscode';
import {LatticeNodeWithData} from './types';

export class LinkNode extends LatticeNodeWithData {
  #link: WadmLink;

  constructor(link: WadmLink) {
    super(link.contract_id, vscode.TreeItemCollapsibleState.Collapsed);
    this.#link = link;
    this.description = link.link_name;
  }

  async getData(): Promise<vscode.TreeItem[]> {
    return [
      buildTreeItem(this.#link, {
        label: 'Link Name',
        contextValue: 'copyText',
        description: (link: WadmLink) => link.link_name,
      }),
      buildTreeItem(this.#link, {
        label: 'Contract',
        contextValue: 'copyText',
        description: (link: WadmLink) => link.contract_id,
      }),
      buildTreeItem(this.#link, {
        label: 'Actor ID',
        contextValue: 'copyText',
        description: (link: WadmLink) => link.actor_id,
      }),
      buildTreeItem(this.#link, {
        label: 'Provider ID',
        contextValue: 'copyText',
        description: (link: WadmLink) => link.provider_id,
      }),
      buildTreeItem(this.#link, {
        label: 'Public Key',
        contextValue: 'copyText',
        description: (link: WadmLink) => link.public_key,
      }),
    ];
  }
}
