// Markdown preview checks run by `FIXTURE=markdown-tests port/tools/webview-check` against the built-in
// markdown-language-features, markdown-math and mermaid-markdown-features.
const assert = require('assert');
const vscode = require('vscode');

const readme = () => vscode.Uri.joinPath(vscode.workspace.workspaceFolders[0].uri, 'README.md');

const activeTab = () => vscode.window.tabGroups.activeTabGroup.activeTab;

async function until(what, predicate) {
	for (let waited = 0; waited < 15000; waited += 100) {
		const value = predicate();
		if (value) {
			return value;
		}
		await new Promise(resolve => setTimeout(resolve, 100));
	}
	throw new Error(`timed out waiting for ${what}`);
}

const openReadme = async () => vscode.window.showTextDocument(await vscode.workspace.openTextDocument(readme()));

const checks = {
	async 'markdown.api.render runs the math and mermaid plugins'() {
		const html = await vscode.commands.executeCommand('markdown.api.render', '# Title\n\n$x^2$\n\n```mermaid\ngraph TD;\nA-->B\n```\n');
		assert.ok(html.includes('<h1'), html);
		assert.ok(html.includes('katex'), html);
		assert.ok(html.includes('mermaid'), html);
	},

	async 'showPreview opens the preview panel of the active file'() {
		await openReadme();
		await vscode.commands.executeCommand('markdown.showPreview');
		const tab = await until('the preview tab', () => {
			const tab = activeTab();
			return tab?.input instanceof vscode.TabInputWebview && tab;
		});
		assert.ok(tab.input.viewType.endsWith('markdown.preview'), tab.input.viewType);
		assert.strictEqual(tab.label, 'Preview README.md');
	},

	async 'showPreviewToSide opens the preview beside the source'() {
		await openReadme();
		await vscode.commands.executeCommand('markdown.showPreviewToSide');
		const tab = await until('the preview tab', () => {
			const tab = activeTab();
			return tab?.input instanceof vscode.TabInputWebview && tab;
		});
		assert.strictEqual(tab.group.viewColumn, vscode.ViewColumn.Two);
	},

	async 'reopenAsPreview opens the preview custom editor'() {
		await openReadme();
		await vscode.commands.executeCommand('markdown.reopenAsPreview');
		const tab = await until('the custom editor tab', () => {
			const tab = activeTab();
			return tab?.input instanceof vscode.TabInputCustom && tab;
		});
		assert.strictEqual(tab.input.viewType, 'vscode.markdown.preview.editor');
		assert.strictEqual(tab.input.uri.toString(), readme().toString());
	},

	async 'reopenAsSource goes back to the text editor'() {
		await vscode.commands.executeCommand('markdown.reopenAsSource');
		const tab = await until('the text tab', () => {
			const tab = activeTab();
			return tab?.input instanceof vscode.TabInputText && tab;
		});
		assert.strictEqual(tab.input.uri.toString(), readme().toString());
	},
};

exports.run = async () => {
	const only = process.env.PORT_WEBVIEW_GREP;
	let failed = 0;
	for (const [name, check] of Object.entries(checks)) {
		if (only && !name.includes(only)) {
			continue;
		}
		try {
			await check();
			console.log(`ok ${name}`);
		} catch (error) {
			failed++;
			console.log(`FAILED ${name}: ${error.message}`);
		}
		if (!name.startsWith('reopenAsPreview')) {
			await vscode.commands.executeCommand('workbench.action.closeAllEditors');
		}
	}
	if (failed) {
		throw new Error(`${failed} markdown checks failed`);
	}
};
