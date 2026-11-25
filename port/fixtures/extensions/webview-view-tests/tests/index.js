// Webview view checks run by `FIXTURE=webview-view-tests port/tools/webview-check`. A contributed Explorer view
// starts collapsed (viewsExtensionPoint.ts `showCollapsed`), so nothing resolves until `<id>.focus` expands it.
const assert = require('assert');
const vscode = require('vscode');

const VIEW_TYPE = 'webviewViewTests.view';

const statefulHtml = `<!DOCTYPE html>
<html lang="en">
<body>
	<script>
		const vscode = acquireVsCodeApi();
		let value = (vscode.getState() || {}).value || 0;
		window.addEventListener('message', (message) => {
			if (message.data.type === 'add') {
				++value;
				vscode.setState({ value });
			}
			vscode.postMessage({ value });
		});
		vscode.postMessage({ type: 'ready' });
	</script>
</body>
</html>`;

const nextMessage = view => new Promise(resolve => {
	const sub = view.webview.onDidReceiveMessage(message => {
		sub.dispose();
		resolve(message);
	});
});

const nextVisibility = view => new Promise(resolve => {
	const sub = view.onDidChangeVisibility(() => {
		sub.dispose();
		resolve(view.visible);
	});
});

const withTimeout = (promise, what) => Promise.race([
	promise,
	new Promise((_, reject) => setTimeout(() => reject(new Error(`timed out waiting for ${what}`)), 15000)),
]);

let view;
let ready;

const checks = {
	async 'the collapsed explorer view resolves once focused'() {
		let resolved = false;
		const resolving = new Promise(resolve => {
			vscode.window.registerWebviewViewProvider(VIEW_TYPE, {
				resolveWebviewView(webviewView) {
					webviewView.webview.options = { enableScripts: true };
					ready = nextMessage(webviewView);
					webviewView.webview.html = statefulHtml;
					resolved = true;
					resolve(webviewView);
				},
			});
		});
		await new Promise(resolve => setTimeout(resolve, 1000));
		assert.strictEqual(resolved, false);
		await vscode.commands.executeCommand(`${VIEW_TYPE}.focus`);
		view = await withTimeout(resolving, 'resolveWebviewView');
		assert.strictEqual(view.viewType, VIEW_TYPE);
		assert.strictEqual(view.title, undefined);
		assert.strictEqual(view.visible, true);
	},

	async 'the view sends and receives messages'() {
		assert.deepStrictEqual(await withTimeout(ready, 'ready'), { type: 'ready' });
		const response = nextMessage(view);
		await view.webview.postMessage({ type: 'add' });
		assert.deepStrictEqual(await withTimeout(response, 'add'), { value: 1 });
	},

	async 'title and description are kept'() {
		view.title = 'Renamed';
		view.description = 'details';
		assert.strictEqual(view.title, 'Renamed');
		assert.strictEqual(view.description, 'details');
	},

	async 'another side bar view hides it and the explorer shows it with its state'() {
		const hidden = nextVisibility(view);
		await vscode.commands.executeCommand('workbench.view.search');
		assert.strictEqual(await withTimeout(hidden, 'hide'), false);
		const shown = nextVisibility(view);
		const reloaded = nextMessage(view);
		await vscode.commands.executeCommand('workbench.view.explorer');
		assert.strictEqual(await withTimeout(shown, 'show'), true);
		assert.deepStrictEqual(await withTimeout(reloaded, 'reload'), { type: 'ready' });
		const response = nextMessage(view);
		await view.webview.postMessage({ type: 'get' });
		assert.deepStrictEqual(await withTimeout(response, 'get'), { value: 1 });
	},

	async 'show reveals the view'() {
		const hidden = nextVisibility(view);
		await vscode.commands.executeCommand('workbench.view.search');
		assert.strictEqual(await withTimeout(hidden, 'hide'), false);
		const shown = nextVisibility(view);
		view.show(true);
		assert.strictEqual(await withTimeout(shown, 'show'), true);
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
	}
	if (failed) {
		throw new Error(`${failed} webview view checks failed`);
	}
};
