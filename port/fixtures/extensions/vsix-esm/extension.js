import * as vscode from 'vscode';

export function activate() {
	globalThis.portVsixEsm = { flag: vscode.workspace.getConfiguration('vsixEsm').get('flag'), known: !!vscode.extensions.getExtension('port.vsix-esm') };
}
