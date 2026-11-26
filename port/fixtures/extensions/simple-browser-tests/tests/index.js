// Simple Browser checks run by `APP=port FIXTURE=simple-browser-tests port/tools/webview-check`. The reference
// registers `workbench.action.browser.open`, so its simple-browser hands every URL to the integrated browser; the
// port removed that browser and the extension opens its own webview panel.
const assert = require('assert');
const http = require('http');
const vscode = require('vscode');

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

/** A page on localhost that reports each request, so a check sees the Simple Browser's frame load it. */
function startServer() {
	const requests = [];
	const server = http.createServer((request, response) => {
		requests.push(request.url);
		response.writeHead(200, { 'Content-Type': 'text/html' });
		response.end('<!DOCTYPE html><title>page</title><h1>Simple Browser fixture</h1>');
	});
	return new Promise(resolve => server.listen(0, '127.0.0.1', () => resolve({ url: `http://127.0.0.1:${server.address().port}/`, requests, server })));
}

const simpleBrowserTab = () => until('the Simple Browser tab', () => {
	const tab = activeTab();
	return tab?.input instanceof vscode.TabInputWebview && tab;
});

const checks = {
	async 'the integrated browser command is not registered'() {
		const commands = await vscode.commands.getCommands(true);
		assert.ok(!commands.includes('workbench.action.browser.open'));
	},

	async 'simpleBrowser.show opens its webview panel and loads the page'() {
		const { url, requests, server } = await startServer();
		try {
			await vscode.commands.executeCommand('simpleBrowser.show', url);
			const tab = await simpleBrowserTab();
			assert.ok(tab.input.viewType.endsWith('simpleBrowser.view'), tab.input.viewType);
			assert.strictEqual(tab.label, 'Simple Browser');
			// preview-src/index.ts navigateTo appends `id` and `vscodeBrowserReqId` to bust the frame's cache.
			await until('the page request', () => requests.some(path => path.startsWith('/?id='))).catch(error => {
				throw new Error(`${error.message}, requests seen: ${JSON.stringify(requests)}`);
			});
		} finally {
			server.close();
		}
	},

	async 'simpleBrowser.api.open opens beside the active editor'() {
		const readme = vscode.Uri.joinPath(vscode.workspace.workspaceFolders[0].uri, 'README.md');
		await vscode.window.showTextDocument(await vscode.workspace.openTextDocument(readme));
		await vscode.commands.executeCommand('simpleBrowser.api.open', vscode.Uri.parse('http://127.0.0.1:9/'), { viewColumn: vscode.ViewColumn.Beside, preserveFocus: true });
		const tab = await until('the Simple Browser tab', () => vscode.window.tabGroups.all.flatMap(group => group.tabs).find(tab => tab.input instanceof vscode.TabInputWebview));
		// SimpleBrowserView's constructor reveals the panel without options afterwards, which focuses it where it is.
		assert.strictEqual(tab.group.viewColumn, vscode.ViewColumn.Two);
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
		await vscode.commands.executeCommand('workbench.action.closeAllEditors');
	}
	if (failed) {
		throw new Error(`${failed} Simple Browser checks failed`);
	}
};
