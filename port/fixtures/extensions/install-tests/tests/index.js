// Installs and uninstalls the VSIX files in PORT_VSIX_DIR through the workbench commands, in the running host.
const assert = require('assert');
const fs = require('fs');
const http = require('http');
const path = require('path');
const vscode = require('vscode');

const vsix = name => vscode.Uri.file(path.join(process.env.PORT_VSIX_DIR, name));
const changed = () => new Promise(resolve => { const listener = vscode.extensions.onDidChange(() => { listener.dispose(); resolve(); }); });
const until = async check => { for (let i = 0; i < 50 && !check(); i++) { await new Promise(resolve => setTimeout(resolve, 100)); } };
const rejection = async run => { try { await run(); } catch (error) { return error.message; } return 'resolved'; };

const checks = {
	async 'an ESM extension activates after install'() {
		assert.strictEqual(vscode.extensions.getExtension('port.vsix-esm'), undefined);
		const didChange = changed();
		await vscode.commands.executeCommand('workbench.extensions.installExtension', vsix('vsix-esm.vsix'));
		await didChange;
		await until(() => globalThis.portVsixEsm);
		assert.deepStrictEqual(globalThis.portVsixEsm, { flag: true, known: true });
		assert.strictEqual(vscode.extensions.getExtension('port.vsix-esm').isActive, true);
	},
	async 'Install from VSIX adds, uninstall removes'() {
		await vscode.commands.executeCommand('workbench.extensions.command.installFromVSIX', [vsix('vsix-cjs.vsix')]);
		assert.ok(vscode.extensions.getExtension('port.vsix-cjs'));
		await vscode.commands.executeCommand('workbench.extensions.uninstallExtension', 'port.vsix-cjs');
		assert.strictEqual(vscode.extensions.getExtension('port.vsix-cjs'), undefined);
	},
	async 'an Open VSX extension installs by id, verified, and activates'() {
		if (!process.env.PORT_GALLERY) {
			return;
		}
		const didChange = changed();
		await vscode.commands.executeCommand('workbench.extensions.installExtension', 'redhat.vscode-yaml');
		await didChange;
		const yaml = vscode.extensions.getExtension('redhat.vscode-yaml');
		assert.ok(yaml);
		await yaml.activate();
		assert.strictEqual(yaml.isActive, true);
		assert.strictEqual(await rejection(() => vscode.commands.executeCommand('workbench.extensions.installExtension', 'nobody.nothing-at-all')), "The extension 'nobody.nothing-at-all' cannot be installed because it was not found.");
		assert.strictEqual(await rejection(() => vscode.commands.executeCommand('workbench.extensions.installExtension', 'redhat.vscode-yaml@0.0.1')), "The extension 'redhat.vscode-yaml' cannot be installed because the requested version '0.0.1' was not found.");
	},
	async '_workbench.downloadResource copies a file and fetches a URL'() {
		const copied = await vscode.commands.executeCommand('_workbench.downloadResource', vsix('vsix-cjs.vsix'));
		assert.deepStrictEqual(fs.readFileSync(copied.fsPath), fs.readFileSync(vsix('vsix-cjs.vsix').fsPath));
		const server = http.createServer((request, response) => {
			response.writeHead(request.url === '/file?x=1' ? 200 : 404);
			response.end(request.url === '/file?x=1' ? 'served' : 'missing');
		});
		await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
		const base = `http://127.0.0.1:${server.address().port}`;
		try {
			const fetched = await vscode.commands.executeCommand('_workbench.downloadResource', vscode.Uri.parse(`${base}/file?x=1`));
			assert.strictEqual(fs.readFileSync(fetched.fsPath, 'utf8'), 'served');
			assert.match(await rejection(() => vscode.commands.executeCommand('_workbench.downloadResource', vscode.Uri.parse(`${base}/gone`))), /^Expected 200, got back 404 instead\./);
		} finally {
			server.close();
		}
	},
	async 'uninstall errors'() {
		assert.strictEqual(await rejection(() => vscode.commands.executeCommand('workbench.extensions.uninstallExtension', 'vscode.git')), "Extension 'vscode.git' is a Built-in extension and cannot be uninstalled");
		assert.strictEqual(await rejection(() => vscode.commands.executeCommand('workbench.extensions.uninstallExtension', 'nobody.nothing')), "Extension 'nobody.nothing' is not installed. Make sure you use the full extension ID, including the publisher, e.g.: ms-dotnettools.csharp.");
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
