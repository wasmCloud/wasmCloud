import * as vscode from 'vscode';
import {LatticeClient, LatticeClientConfig} from '@wasmcloud/lattice-client-core';

let client: LatticeClient;

export async function getLatticeClient(): Promise<LatticeClient | null> {
  if (client) {
    return client;
  }

  const config = getExtensionConfig();

  vscode.workspace.onDidChangeConfiguration((event) => {
    if (event.affectsConfiguration('wasmCloud')) {
      const newConfig = getExtensionConfig();
      client.setPartialConfig(newConfig);
    }
  });

  client = new LatticeClient({ config });

  try {
    await connect();
  } catch (error) {
    return null;
  }
  
  return client;
}

type ExtensionConfig = Required<Pick<LatticeClientConfig, 'latticeUrl'>> & Partial<Omit<LatticeClientConfig, 'latticeUrl'>>;

function getExtensionConfig(): ExtensionConfig {
  const config = vscode.workspace.getConfiguration('wasmCloud');
  const latticeUrl = config.get<string>('latticeUrl') || 'ws://localhost:4001';
  const ctlTopicPrefix = config.get<string>('ctlTopicPrefix') || undefined;
  const latticeId = config.get<string>('latticeId') || undefined;

  return {latticeUrl, ctlTopicPrefix, latticeId};
}

async function connect() {
  console.log('Connecting to lattice...');
  vscode.commands.executeCommand('setContext', 'wasmCloud.lattice.isLoading', true);

  try {
    await client.disconnect();
    await client.connect();
    vscode.commands.executeCommand('setContext', 'wasmCloud.lattice.isLoading', false);
    vscode.commands.executeCommand('setContext', 'wasmCloud.lattice.isConnected', true);
  } catch (error) {
    if (error instanceof Error) {
      // log error
    } else {
      console.log(error);
    }
    vscode.commands.executeCommand('setContext', 'wasmCloud.lattice.isLoading', false);
    vscode.commands.executeCommand('setContext', 'wasmCloud.lattice.isConnected', false);
  }
}

export async function reconnect() {
  await connect();
}