import * as vscode from 'vscode';
import WebSocket from 'ws';
import {LatticeDataProvider} from '@/lattice/LatticeDataProvider';
import {copyToClipboard} from '@/commands/copyToClipboard';
import {getLatticeClient, reconnect} from '@/lattice/client';
import { openSettings } from './commands/openSettings';

export async function activate(context: vscode.ExtensionContext) {
  // TODO: This is a hack because nats.ws doesn't have a method of providing the WebSocket implementation directly
  Object.assign(globalThis, {WebSocket});

  const latticeDataProvider = new LatticeDataProvider();
  vscode.window.createTreeView('wasmCloudExplorer', {treeDataProvider: latticeDataProvider});
  vscode.commands.executeCommand('setContext', 'wasmCloud.lattice.isLoading', true);
  vscode.commands.executeCommand('setContext', 'wasmCloud.lattice.isConnected', false);

  context.subscriptions.push(
    vscode.commands.registerCommand('wasmCloud.copyToClipboard', copyToClipboard),
    vscode.commands.registerCommand('wasmCloud.openSettings', openSettings),
    vscode.commands.registerCommand('wasmCloud.reconnect', reconnect),
  );
}

export async function deactivate() {
  const client = await getLatticeClient();

  if (!client) {
    return;
  }

  client.disconnect();
}
