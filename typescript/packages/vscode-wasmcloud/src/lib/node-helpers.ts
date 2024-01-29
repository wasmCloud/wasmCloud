import * as vscode from 'vscode';

type TreeItemBuilderOptions<T> = {
  label: ((provider: T) => string) | string;
  description: ((provider: T) => string) | string;
  contextValue: ((provider: T) => string) | string;
};

export function buildTreeItem<T>(
  element: T,
  {label, description, contextValue}: TreeItemBuilderOptions<T>,
): vscode.TreeItem {
  const labelValue = typeof label === 'function' ? label(element) : label;
  const descriptionValue = typeof description === 'function' ? description(element) : description;
  const contextValueString =
    typeof contextValue === 'function' ? contextValue(element) : contextValue;

  const node = new vscode.TreeItem(labelValue, vscode.TreeItemCollapsibleState.None);
  node.description = descriptionValue;
  node.contextValue = contextValueString;

  return node;
}
