// Editable custom documents (MainThreadCustomEditorModel): dirty state, undo and redo, save and revert through
// the provider; run by port/tools/webview-check (FIXTURE=custom-document-tests) in both apps.
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vscode = require('vscode');

const root = vscode.Uri.file(path.join(vscode.workspace.workspaceFolders[0].uri.fsPath, 'customDocuments'));
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
const run = (command, ...args) => vscode.commands.executeCommand(command, ...args);
const providerLog = () => run('_cdoc.log');

function activeTab() {
	return vscode.window.tabGroups.activeTabGroup.activeTab;
}

async function waitFor(condition, what) {
	for (let tries = 0; tries < 100; tries++) {
		if (await condition()) {
			return;
		}
		await delay(50);
	}
	throw new Error(`timed out waiting for ${what}`);
}

async function openDocument(contents) {
	const file = vscode.Uri.joinPath(root, `${Math.random().toString(36).slice(2, 12)}.cdoc`);
	await fs.promises.writeFile(file.fsPath, contents);
	await run('vscode.openWith', file, 'testCustomDocument.cdoc');
	await waitFor(() => activeTab()?.input?.viewType === 'testCustomDocument.cdoc', 'the custom editor tab');
	await providerLog();
	return file;
}

const checks = {
	async 'An edit makes the document dirty, undo and redo go through the provider'() {
		await openDocument('a');
		await run('_cdoc.edit', 'b');
		await waitFor(() => activeTab().isDirty, 'dirty after an edit');
		await run('undo');
		await waitFor(() => !activeTab().isDirty, 'clean after undo');
		await run('redo');
		await waitFor(() => activeTab().isDirty, 'dirty after redo');
		assert.deepStrictEqual(await providerLog(), ['undo b', 'redo b']);
	},
	async 'Save writes through the provider and the document is clean'() {
		const file = await openDocument('a');
		await run('_cdoc.edit', 'b');
		await waitFor(() => activeTab().isDirty, 'dirty after an edit');
		await run('workbench.action.files.save');
		await waitFor(() => !activeTab().isDirty, 'clean after save');
		assert.deepStrictEqual(await providerLog(), ['save']);
		assert.strictEqual(await fs.promises.readFile(file.fsPath, 'utf8'), 'a\nb');
	},
	async 'Revert goes through the provider and drops the edits after the save point'() {
		await openDocument('a');
		await run('_cdoc.edit', 'b');
		await waitFor(() => activeTab().isDirty, 'dirty after an edit');
		await run('workbench.action.files.revert');
		await waitFor(() => !activeTab().isDirty, 'clean after revert');
		await run('undo');
		await delay(200);
		assert.deepStrictEqual(await providerLog(), ['revert']);
	},
	async 'A content change makes the document dirty without an undo stop'() {
		await openDocument('a');
		await run('_cdoc.contentChange');
		await waitFor(() => activeTab().isDirty, 'dirty after a content change');
		await run('undo');
		await delay(200);
		assert.ok(activeTab().isDirty);
		assert.deepStrictEqual(await providerLog(), []);
		await run('workbench.action.files.save');
		await waitFor(() => !activeTab().isDirty, 'clean after save');
	},
};

exports.run = async () => {
	const only = process.env.PORT_WEBVIEW_GREP;
	let failed = 0;
	await fs.promises.mkdir(root.fsPath, { recursive: true });
	for (const [name, check] of Object.entries(checks)) {
		if (only && !name.includes(only)) {
			continue;
		}
		try {
			await Promise.race([check(), new Promise((_, reject) => setTimeout(() => reject(new Error('timed out after 30 s')), 30000))]);
			console.log(`ok ${name}`);
		} catch (error) {
			failed++;
			console.log(`FAILED ${name}: ${error.message}`);
		}
		await run('workbench.action.closeAllEditors');
	}
	if (failed) {
		throw new Error(`${failed} failed`);
	}
};
