import {WadmHost} from '@wasmcloud/lattice-client-core';
import * as vscode from 'vscode';

export class SimpleHostNode extends vscode.TreeItem {
  contextValue = 'goToHost';

  constructor(host: WadmHost) {
    super(host.friendly_name, vscode.TreeItemCollapsibleState.None);
    this.description = host.id;
  }
}
