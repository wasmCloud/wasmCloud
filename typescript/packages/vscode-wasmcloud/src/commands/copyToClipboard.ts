import * as vscode from 'vscode';

export function copyToClipboard(element: vscode.TreeItem | string): void {
  let value: string = '';
  if (typeof element === 'string') {
    value = element;
  } else if ('copyValue' in element) {
    value = element['copyValue'] as string;
  } else {
    value = element.description?.toString() ?? element.label?.toString() ?? '';
  }

  if (!value) {
    vscode.window.showInformationMessage('Nothing to copy');
    return;
  }

  vscode.env.clipboard.writeText(value);
  vscode.window.showInformationMessage('Copied to clipboard');
}
