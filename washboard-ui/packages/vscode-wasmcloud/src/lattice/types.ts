import * as vscode from 'vscode';
import {ActorNode} from './ActorNode';
import {ActorsNode} from './ActorsNode';
import {HostNode} from './HostNode';
import {HostsNode} from './HostsNode';
import {LinksNode} from './LinksNode';
import {ProviderNode} from './ProviderNode';
import {ProvidersNode} from './ProvidersNode';
import {SimpleHostNode} from './SimpleHostNode';
import {SimpleHostsNode} from './SimpleHostsNode';

export type LatticeExplorerNode =
  | HostsNode
  | HostNode
  | SimpleHostNode
  | SimpleHostsNode
  | ProvidersNode
  | ProviderNode
  | ActorsNode
  | ActorNode
  | LinksNode;

export type LatticeExplorerRootNode = ProvidersNode | HostsNode | ActorsNode | LinksNode;

export abstract class LatticeNodeWithData extends vscode.TreeItem {
  abstract getData(): Promise<vscode.TreeItem[]>;
}

export abstract class LatticeNodeWithChildren extends vscode.TreeItem {
  abstract getChildren(): Promise<LatticeNodeWithData[] | vscode.TreeItem[]>;
}
