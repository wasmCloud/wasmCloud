import {WadmActor} from '@wasmcloud/lattice-client-core';
import * as vscode from 'vscode';
import {buildTreeItem} from '@/lib/node-helpers';
import {SimpleHostsNode} from './SimpleHostsNode';
import {LatticeNodeWithData} from './types';

export class ActorNode extends LatticeNodeWithData {
  #actor: WadmActor;

  constructor(actor: WadmActor) {
    super(actor.name, vscode.TreeItemCollapsibleState.Collapsed);
    this.description = actor.reference;
    this.#actor = actor;
  }

  async getData(): Promise<vscode.TreeItem[]> {
    return Promise.resolve([
      buildTreeItem(this.#actor, {
        label: 'Name',
        contextValue: 'copyText',
        description: (actor: WadmActor) => actor.name,
      }),
      buildTreeItem(this.#actor, {
        label: 'Reference',
        contextValue: 'copyText',
        description: (actor: WadmActor) => actor.reference,
      }),
      buildTreeItem(this.#actor, {
        label: 'Issuer',
        contextValue: 'copyText',
        description: (actor: WadmActor) => actor.issuer,
      }),
      buildTreeItem(this.#actor, {
        label: 'ID',
        contextValue: 'copyText',
        description: (actor: WadmActor) => actor.id,
      }),
      new SimpleHostsNode(Object.keys(this.#actor.instances)),
    ]);
  }
}
