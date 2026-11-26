// An editable CustomEditorProvider for *.cdoc whose document is a list of lines: `_cdoc.edit` appends one as an
// undoable edit, `_cdoc.contentChange` marks the document changed, `_cdoc.log` answers the provider calls so far.
const vscode = require('vscode');

const log = [];

exports.activate = context => {
	const onDidChangeCustomDocument = new vscode.EventEmitter();
	let active;
	const read = async uri => new TextDecoder().decode(await vscode.workspace.fs.readFile(uri)).split('\n').filter(Boolean);
	const provider = {
		onDidChangeCustomDocument: onDidChangeCustomDocument.event,
		async openCustomDocument(uri, openContext) {
			log.push(`open ${openContext.backupId ?? ''}`);
			const document = { uri, lines: await read(uri), dispose() { } };
			return document;
		},
		resolveCustomEditor(document, panel) {
			active = document;
			panel.webview.html = '<!DOCTYPE html><html><body>cdoc</body></html>';
		},
		async saveCustomDocument(document) {
			log.push('save');
			await vscode.workspace.fs.writeFile(document.uri, new TextEncoder().encode(document.lines.join('\n')));
		},
		async saveCustomDocumentAs(document, destination) {
			log.push('saveAs');
			await vscode.workspace.fs.writeFile(destination, new TextEncoder().encode(document.lines.join('\n')));
		},
		async revertCustomDocument(document) {
			log.push('revert');
			document.lines = await read(document.uri);
		},
		async backupCustomDocument(document, backupContext) {
			log.push('backup');
			return { id: backupContext.destination.toString(), delete() { } };
		},
	};
	context.subscriptions.push(
		vscode.window.registerCustomEditorProvider('testCustomDocument.cdoc', provider),
		vscode.commands.registerCommand('_cdoc.edit', line => {
			const document = active;
			document.lines.push(line);
			onDidChangeCustomDocument.fire({
				document,
				label: 'Add line',
				undo: () => { log.push(`undo ${line}`); document.lines.pop(); },
				redo: () => { log.push(`redo ${line}`); document.lines.push(line); },
			});
		}),
		vscode.commands.registerCommand('_cdoc.contentChange', () => onDidChangeCustomDocument.fire({ document: active })),
		vscode.commands.registerCommand('_cdoc.log', () => log.splice(0)),
	);
};
