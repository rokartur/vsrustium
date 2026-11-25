const fs = require('fs');
const vscode = require('vscode');
exports.activate = () => { };
exports.run = async () => {
	const out = [];
	for (const uri of ['vscode://defaultsettings/defaultSettings.jsonc', 'vscode://defaultsettings/keybindings.json']) {
		try {
			const document = await vscode.workspace.openTextDocument(vscode.Uri.parse(uri));
			out.push(`== ${uri} ${document.languageId}`, document.getText());
		} catch (error) {
			out.push(`== ${uri} failed: ${error.message}`);
		}
	}
	// The settings schemas json-language-features reads: which keys each one validates.
	for (const name of ['resourceLanguage', 'default', 'user', 'profile', 'machine', 'workspace', 'folder', 'configurationDefaults']) {
		const uri = `vscode://schemas/settings/${name}`;
		try {
			const schema = JSON.parse(new TextDecoder().decode(await vscode.workspace.fs.readFile(vscode.Uri.parse(uri))));
			out.push(`== ${uri}`, ...Object.keys(schema.properties ?? {}).sort(), `patternProperties: ${Object.keys(schema.patternProperties ?? {}).join(' ')}`);
		} catch (error) {
			out.push(`== ${uri} failed: ${error.message}`);
		}
	}
	const associations = 'vscode://schemas-associations/schemas-associations.json';
	out.push(`== ${associations}`, new TextDecoder().decode(await vscode.workspace.fs.readFile(vscode.Uri.parse(associations))));
	// json-language-features validates a folder settings.json against vscode://schemas/settings/folder.
	const settings = vscode.Uri.file(`${process.env.CHECK_DIR}/folder/.vscode/settings.json`);
	fs.mkdirSync(`${process.env.CHECK_DIR}/folder/.vscode`, { recursive: true });
	fs.writeFileSync(settings.fsPath, '{\n\t"editor.fontSize": "big",\n\t"unknown.setting": 1,\n\t"window.zoomLevel": 1\n}\n');
	await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(settings));
	let diagnostics = [];
	for (let waited = 0; waited < 30000 && diagnostics.length < 3; waited += 250) {
		await new Promise(resolve => setTimeout(resolve, 250));
		diagnostics = vscode.languages.getDiagnostics(settings);
	}
	out.push('== diagnostics of .vscode/settings.json', ...diagnostics.map(d => `${d.range.start.line}:${d.range.start.character} ${d.severity} ${d.message}`).sort());
	fs.writeFileSync(process.env.CHECK_OUT, out.join('\n') + '\n');
};
