import * as vscode from 'vscode';

let logger: vscode.OutputChannel;

export function init() {
  logger = vscode.window.createOutputChannel('wasmCloud');
}

export function log(...values: unknown[]) {
  logger.appendLine(values.join(' '));
}

export function logDebug(...values: unknown[]) {
  logger.appendLine(values.join(' '));
}

export function logError(e: Error, ...values: unknown[]) {
  logger.appendLine(values.join(' '));
  logger.appendLine(e.message);
  if (e.stack) {
    logger.appendLine(e.stack);
  }
}

export function revealLog() {
  logger.show();
}
