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
	fs.writeFileSync(process.env.CHECK_OUT, out.join('\n') + '\n');
};
