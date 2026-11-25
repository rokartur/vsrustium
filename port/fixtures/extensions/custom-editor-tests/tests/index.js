// The tests of extensions/vscode-custom-editor-tests/src/test/customEditor.test.ts, which upstream removed in
// bbe7b54cf4d, run by port/tools/webview-check (FIXTURE=custom-editor-tests). The two upstream skipped stay out.
const assert = require('assert');
const fs = require('fs');
const path = require('path');
const vscode = require('vscode');

const testWorkspaceRoot = vscode.Uri.file(path.join(vscode.workspace.workspaceFolders[0].uri.fsPath, 'customEditors'));
const commands = { open: 'vscode.open', save: 'workbench.action.files.save', undo: 'undo', type: '_abcEditor.type', contentChange: '_abcEditor.contentChange' };

const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
const closeAllEditors = () => vscode.commands.executeCommand('workbench.action.closeAllEditors');

async function writeRandomFile(contents) {
	const name = Math.random().toString(36).slice(2, 12);
	const file = vscode.Uri.joinPath(testWorkspaceRoot, `${name}.abc`);
	await fs.promises.writeFile(file.fsPath, contents);
	return file;
}

async function resetTestWorkspace() {
	try {
		await vscode.workspace.fs.delete(testWorkspaceRoot, { recursive: true });
	} catch {
		// ok if file doesn't exist
	}
	await vscode.workspace.fs.createDirectory(testWorkspaceRoot);
}

const disposables = [];

function createUpdateListener() {
	const unconsumed = [];
	const callbacks = [];
	disposables.push(vscode.commands.registerCommand(commands.contentChange, data => {
		if (callbacks.length) {
			callbacks.shift()(data);
		} else {
			unconsumed.push(data);
		}
	}));
	return { nextResponse: () => unconsumed.length ? Promise.resolve(unconsumed.shift()) : new Promise(resolve => callbacks.push(resolve)) };
}

const checks = {
	async 'Should load basic content from disk'() {
		const startingContent = `load, init`;
		const testDocument = await writeRandomFile(startingContent);
		const listener = createUpdateListener();
		await vscode.commands.executeCommand(commands.open, testDocument);
		const { content } = await listener.nextResponse();
		assert.strictEqual(content, startingContent);
	},
	async 'Should support basic edits'() {
		const testDocument = await writeRandomFile(`basic edit, init`);
		const listener = createUpdateListener();
		await vscode.commands.executeCommand(commands.open, testDocument);
		await listener.nextResponse();
		const newContent = `basic edit test`;
		await vscode.commands.executeCommand(commands.type, newContent);
		const { content } = await listener.nextResponse();
		assert.strictEqual(content, newContent);
	},
	async 'Should support single undo'() {
		const startingContent = `single undo, init`;
		const testDocument = await writeRandomFile(startingContent);
		const listener = createUpdateListener();
		await vscode.commands.executeCommand(commands.open, testDocument);
		await listener.nextResponse();
		const newContent = `undo test`;
		await vscode.commands.executeCommand(commands.type, newContent);
		assert.strictEqual((await listener.nextResponse()).content, newContent);
		await delay(100);
		await vscode.commands.executeCommand(commands.undo);
		assert.strictEqual((await listener.nextResponse()).content, startingContent);
	},
	async 'Should support multiple undo'() {
		const startingContent = `multiple undo, init`;
		const testDocument = await writeRandomFile(startingContent);
		const listener = createUpdateListener();
		await vscode.commands.executeCommand(commands.open, testDocument);
		await listener.nextResponse();
		const count = 10;
		for (let i = 0; i < count; ++i) {
			await vscode.commands.executeCommand(commands.type, `${i}`);
			assert.strictEqual(`${i}`, (await listener.nextResponse()).content);
		}
		for (let i = count - 1; i; --i) {
			await delay(100);
			await vscode.commands.executeCommand(commands.undo);
			assert.strictEqual(`${i - 1}`, (await listener.nextResponse()).content);
		}
		await delay(100);
		await vscode.commands.executeCommand(commands.undo);
		assert.strictEqual((await listener.nextResponse()).content, startingContent);
	},
	async 'Should update custom editor on file move'() {
		const startingContent = `file move, init`;
		const testDocument = await writeRandomFile(startingContent);
		const listener = createUpdateListener();
		await vscode.commands.executeCommand(commands.open, testDocument);
		await listener.nextResponse();
		const newFileName = vscode.Uri.file(path.join(testWorkspaceRoot.fsPath, 'y.abc'));
		const edit = new vscode.WorkspaceEdit();
		edit.renameFile(testDocument, newFileName);
		await vscode.workspace.applyEdit(edit);
		const response = await listener.nextResponse();
		assert.strictEqual(response.content, startingContent);
		assert.strictEqual(response.source.toString(), newFileName.toString());
	},
	async 'Should support saving custom editors'() {
		const testDocument = await writeRandomFile(`save, init`);
		const listener = createUpdateListener();
		await vscode.commands.executeCommand(commands.open, testDocument);
		await listener.nextResponse();
		const newContent = `save, new`;
		await vscode.commands.executeCommand(commands.type, newContent);
		assert.strictEqual((await listener.nextResponse()).content, newContent);
		await vscode.commands.executeCommand(commands.save);
		assert.strictEqual((await fs.promises.readFile(testDocument.fsPath)).toString(), newContent);
	},
	async 'Should undo after saving custom editor'() {
		const startingContent = `undo after save, init`;
		const testDocument = await writeRandomFile(startingContent);
		const listener = createUpdateListener();
		await vscode.commands.executeCommand(commands.open, testDocument);
		await listener.nextResponse();
		const newContent = `undo after save, new`;
		await vscode.commands.executeCommand(commands.type, newContent);
		assert.strictEqual((await listener.nextResponse()).content, newContent);
		await vscode.commands.executeCommand(commands.save);
		assert.strictEqual((await fs.promises.readFile(testDocument.fsPath)).toString(), newContent);
		await delay(100);
		await vscode.commands.executeCommand(commands.undo);
		assert.strictEqual((await listener.nextResponse()).content, startingContent);
	},
	async 'Should release the text document when the editor is closed'() {
		const testDocument = await writeRandomFile(`release document init,`);
		const listener = createUpdateListener();
		await vscode.commands.executeCommand(commands.open, testDocument);
		await listener.nextResponse();
		const doc = vscode.workspace.textDocuments.find(x => x.uri.toString() === testDocument.toString());
		assert.ok(doc);
		assert.ok(!doc.isClosed);
		await closeAllEditors();
		await delay(100);
		assert.ok(doc.isClosed);
	},
};

exports.run = async () => {
	const only = process.env.PORT_WEBVIEW_GREP;
	let failed = 0;
	for (const [name, check] of Object.entries(checks)) {
		if (only && !name.includes(only)) {
			continue;
		}
		await closeAllEditors();
		await resetTestWorkspace();
		try {
			await Promise.race([check(), new Promise((_, reject) => setTimeout(() => reject(new Error('timed out after 30 s')), 30000))]);
			console.log(`ok ${name}`);
		} catch (error) {
			failed++;
			console.log(`FAILED ${name}: ${error.message}`);
		}
		await closeAllEditors();
		disposables.splice(0).forEach(disposable => disposable.dispose());
		await resetTestWorkspace();
	}
	if (failed) {
		throw new Error(`${failed} failed`);
	}
};
