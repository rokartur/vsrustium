// Installs and uninstalls the VSIX files in PORT_VSIX_DIR through the workbench commands, in the running host.
const assert = require('assert');
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
