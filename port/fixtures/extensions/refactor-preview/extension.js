// At startup: replaces 42 in src/util.ts and src/main.ts and creates src/answer.ts, in two categories that need
// confirmation, so the edit waits in the Refactor Preview.
const vscode = require('vscode');

exports.activate = async function () {
	const folder = vscode.workspace.workspaceFolders?.[0];
	if (!folder) {
		return;
	}
	const edit = new vscode.WorkspaceEdit();
	const bump = { label: 'Change the answer', description: '42 to 43', needsConfirmation: true };
	for (const name of ['src/util.ts', 'src/main.ts']) {
		const uri = vscode.Uri.joinPath(folder.uri, name);
		const document = await vscode.workspace.openTextDocument(uri);
		const text = document.getText();
		for (let index = text.indexOf('42'); index !== -1; index = text.indexOf('42', index + 1)) {
			edit.replace(uri, new vscode.Range(document.positionAt(index), document.positionAt(index + 2)), '43', bump);
		}
	}
	edit.createFile(vscode.Uri.joinPath(folder.uri, 'src/answer.ts'), { ignoreIfExists: true }, { label: 'Create answer.ts', needsConfirmation: true });
	const applied = await vscode.workspace.applyEdit(edit);
	console.log(`refactor-preview: ${applied}`);
};
