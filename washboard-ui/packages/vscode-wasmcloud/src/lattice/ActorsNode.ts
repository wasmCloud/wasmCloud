import * as vscode from 'vscode';
import {ActorNode} from './ActorNode';
import {WadmActor} from '@wasmcloud/lattice-client-core';
import {sortById} from '@/lib/sort';
import {LatticeNodeWithChildren} from '@/lattice/types';

export class ActorsNode extends LatticeNodeWithChildren {
  #actors: WadmActor[];

  constructor(actors: Record<string, WadmActor>) {
    const count = Object.keys(actors).length;
    super(
      'Actors',
      count > 0 ? vscode.TreeItemCollapsibleState.Collapsed : vscode.TreeItemCollapsibleState.None,
    );
    this.#actors = sortById(actors);
    this.description = count.toString();
  }

  async getChildren(): Promise<ActorNode[]> {
    return Promise.resolve(this.#actors.map((actor) => new ActorNode(actor)));
  }
}
