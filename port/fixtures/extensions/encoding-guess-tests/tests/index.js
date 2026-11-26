// files.autoGuessEncoding in the packaged app: the guess worker runs on the bundle's bun and node_modules.
// port/tools/app-smoke copies encoding.test.ts's fixtures into the opened folder and turns the setting on.
const assert = require('assert');
const vscode = require('vscode');

const open = name => vscode.workspace.openTextDocument(vscode.Uri.joinPath(vscode.workspace.workspaceFolders[0].uri, name));

const checks = {
	async 'a Shift JIS file is guessed as shiftjis'() {
		assert.strictEqual((await open('some.shiftjis.txt')).encoding, 'shiftjis');
	},
	async 'a windows-1252 file is guessed as windows1252'() {
		assert.strictEqual((await open('some.cp1252.txt')).encoding, 'windows1252');
	},
};

exports.run = async () => {
	let failed = 0;
	for (const [name, check] of Object.entries(checks)) {
		try {
			await check();
			console.log(`ok ${name}`);
		} catch (error) {
			failed++;
			console.log(`FAILED ${name}: ${error.message}`);
		}
	}
	if (failed) {
		throw new Error(`${failed} failed`);
	}
};
