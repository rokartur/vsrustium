const vscode = require('vscode');
const fs = require('fs');
const path = require('path');
exports.activate = () => { };
exports.run = async () => {
	const out = [];
	const user = path.join(process.env.CHECK_USER_DIR, 'User', 'settings.json');
	const folder = vscode.workspace.workspaceFolders[0].uri.fsPath;
	const read = file => fs.existsSync(file) ? fs.readFileSync(file, 'utf8') : null;
	const events = [];
	vscode.workspace.onDidChangeConfiguration(e => events.push(['editor.tabSize', 'editor.fontSize', 'editor.wordWrap', 'configCheck.app'].filter(k => e.affectsConfiguration(k))));
	const step = async (name, fn) => {
		events.length = 0;
		let result;
		try { result = await fn(); } catch (e) { result = 'error: ' + e.message; }
		out.push({ name, result, events: events.slice() });
	};
	await step('global tabSize 3', async () => { await vscode.workspace.getConfiguration('editor').update('tabSize', 3, vscode.ConfigurationTarget.Global); return [vscode.workspace.getConfiguration('editor').get('tabSize'), read(user)]; });
	await step('workspace wordWrap on', async () => { await vscode.workspace.getConfiguration('editor').update('wordWrap', 'on', vscode.ConfigurationTarget.Workspace); return [vscode.workspace.getConfiguration('editor').inspect('wordWrap').workspaceValue, read(path.join(folder, '.vscode', 'settings.json'))]; });
	await step('markdown tabSize 7', async () => { await vscode.workspace.getConfiguration('editor', { languageId: 'markdown' }).update('tabSize', 7, vscode.ConfigurationTarget.Global, true); return [vscode.workspace.getConfiguration('editor', { languageId: 'markdown' }).get('tabSize'), read(user)]; });
	await step('remove tabSize', async () => { await vscode.workspace.getConfiguration('editor').update('tabSize', undefined, vscode.ConfigurationTarget.Global); return [vscode.workspace.getConfiguration('editor').get('tabSize'), read(user)]; });
	await step('application in workspace', async () => vscode.workspace.getConfiguration('configCheck').update('app', 'x', vscode.ConfigurationTarget.Workspace));
	await step('unknown key', async () => vscode.workspace.getConfiguration().update('no.such.key', 1, vscode.ConfigurationTarget.Global));
	await step('external edit', async () => {
		fs.writeFileSync(user, '{\n\t"editor.fontSize": 20\n}');
		for (let i = 0; i < 100 && !events.length; i++) { await new Promise(r => setTimeout(r, 50)); }
		return vscode.workspace.getConfiguration('editor').get('fontSize');
	});
	fs.writeFileSync(process.env.CHECK_OUT, JSON.stringify(out, null, 1));
};
