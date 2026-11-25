// Webview checks run in code-gpui by port/tools/webview-check. The panel tests are the ones upstream removed from
// extensions/vscode-api-tests/src/singlefolder-tests/webview.test.ts in c5f857f0cf0 (skipped there as flaky in CI).
const assert = require('assert');
const vscode = require('vscode');

const disposables = [];
const register = disposable => (disposables.push(disposable), disposable);
const workspaceFile = (...segments) => vscode.Uri.joinPath(vscode.workspace.workspaceFolders[0].uri, ...segments);
const createPanel = options => register(vscode.window.createWebviewPanel('myWebview', 'title', { viewColumn: vscode.ViewColumn.One }, options));

const html = body => `<!DOCTYPE html>
<html lang="en">
<head>
	<meta charset="UTF-8">
	<meta name="viewport" content="width=device-width, initial-scale=1.0">
	<title>Document</title>
</head>
<body>
	${body}
</body>
</html>`;

const statefulHtml = html(`
	<script>
		const vscode = acquireVsCodeApi();
		let value = (vscode.getState() || {}).value || 0;
		window.addEventListener('message', (message) => {
			switch (message.data.type) {
				case 'get':
					vscode.postMessage({ value });
					break;
				case 'add':
					++value;
					vscode.setState({ value });
					vscode.postMessage({ value });
					break;
			}
		});
		vscode.postMessage({ type: 'ready' });
	</script>`);

const getMessage = webview => new Promise(resolve => {
	const sub = webview.webview.onDidReceiveMessage(message => {
		sub.dispose();
		resolve(message);
	});
});

const sendReceiveMessage = (webview, message) => {
	const response = getMessage(webview);
	webview.webview.postMessage(message);
	return response;
};

const showOtherEditor = async () => vscode.window.showTextDocument(await vscode.workspace.openTextDocument(workspaceFile('notes.txt')));

const checks = {
	async 'webviews should be able to send and receive messages'() {
		const webview = createPanel({ enableScripts: true });
		const firstResponse = getMessage(webview);
		webview.webview.html = html(`
			<script>
				const vscode = acquireVsCodeApi();
				window.addEventListener('message', (message) => {
					vscode.postMessage({ value: message.data.value + 1 });
				});
			</script>`);
		webview.webview.postMessage({ value: 1 });
		assert.strictEqual((await firstResponse).value, 2);
	},
	async 'webviews should not have scripts enabled by default'() {
		const webview = createPanel({});
		const response = Promise.race([getMessage(webview), new Promise(resolve => setTimeout(() => resolve({ value: 'none' }), 1000))]);
		webview.webview.html = html(`
			<script>
				const vscode = acquireVsCodeApi();
				vscode.postMessage({ value: 'script ran' });
			</script>`);
		assert.strictEqual((await response).value, 'none');
	},
	async 'webviews should update html'() {
		const webview = createPanel({ enableScripts: true });
		for (const value of ['first', 'second']) {
			const response = getMessage(webview);
			webview.webview.html = html(`
				<script>
					const vscode = acquireVsCodeApi();
					vscode.postMessage({ value: '${value}' });
				</script>`);
			assert.strictEqual((await response).value, value);
		}
	},
	async 'webviews should preserve vscode API state when they are hidden'() {
		const webview = createPanel({ enableScripts: true });
		const ready = getMessage(webview);
		webview.webview.html = statefulHtml;
		await ready;
		assert.strictEqual((await sendReceiveMessage(webview, { type: 'add' })).value, 1);
		await showOtherEditor();
		const ready2 = getMessage(webview);
		webview.reveal(vscode.ViewColumn.One);
		await ready2;
		assert.strictEqual((await sendReceiveMessage(webview, { type: 'get' })).value, 1);
	},
	async 'webviews with retainContextWhenHidden should be able to receive messages while hidden'() {
		const webview = createPanel({ enableScripts: true, retainContextWhenHidden: true });
		const ready = getMessage(webview);
		webview.webview.html = statefulHtml;
		await ready;
		assert.strictEqual((await sendReceiveMessage(webview, { type: 'add' })).value, 1);
		await showOtherEditor();
		assert.strictEqual((await sendReceiveMessage(webview, { type: 'add' })).value, 2);
		webview.reveal(vscode.ViewColumn.One);
		assert.strictEqual((await sendReceiveMessage(webview, { type: 'get' })).value, 2);
	},
	async 'webviews should transfer ArrayBuffers to and from webviews'() {
		const webview = createPanel({ enableScripts: true, retainContextWhenHidden: true });
		const ready = getMessage(webview);
		webview.webview.html = html(`
			<script>
				const vscode = acquireVsCodeApi();
				window.addEventListener('message', (message) => {
					const arrayBuffer = message.data.array;
					const uint8Array = new Uint8Array(arrayBuffer);
					for (let i = 0; i < uint8Array.length; ++i) {
						uint8Array[i] = uint8Array[i] + 1;
					}
					vscode.postMessage({ array: arrayBuffer }, [arrayBuffer]);
				});
				vscode.postMessage({ type: 'ready' });
			</script>`);
		await ready;
		const responsePromise = getMessage(webview);
		const arrayBuffer = new ArrayBuffer(100);
		new Uint8Array(arrayBuffer).forEach((_, i, array) => array[i] = i);
		webview.webview.postMessage({ type: 'add1', array: arrayBuffer });
		const response = await responsePromise;
		assert.ok(response.array instanceof ArrayBuffer);
		new Uint8Array(response.array).forEach((value, i) => assert.strictEqual(value, i + 1));
	},
	async 'webviews should transfer Typed arrays to and from webviews'() {
		const webview = createPanel({ enableScripts: true, retainContextWhenHidden: true });
		const ready = getMessage(webview);
		webview.webview.html = html(`
			<script>
				const vscode = acquireVsCodeApi();
				window.addEventListener('message', (message) => {
					const uint8Array = message.data.array1;
					const uint16Array = message.data.array2;
					for (let i = 0; i < uint16Array.length; ++i) {
						uint16Array[i] = uint16Array[i] + 1;
					}
					vscode.postMessage({ array1: uint8Array, array2: uint16Array }, [uint16Array.buffer]);
				});
				vscode.postMessage({ type: 'ready' });
			</script>`);
		await ready;
		const responsePromise = getMessage(webview);
		const arrayBuffer = new ArrayBuffer(100);
		const uint16Array = new Uint16Array(arrayBuffer);
		uint16Array.forEach((_, i) => uint16Array[i] = i);
		webview.webview.postMessage({ type: 'add1', array1: new Uint8Array(arrayBuffer), array2: uint16Array });
		const response = await responsePromise;
		assert.ok(response.array1 instanceof Uint8Array);
		assert.ok(response.array2 instanceof Uint16Array);
		assert.ok(response.array1.buffer === response.array2.buffer);
		response.array1.forEach((value, i) => assert.strictEqual(value, i % 2 === 0 ? Math.floor(i / 2) + 1 : 0));
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
			await Promise.race([check(), new Promise((_, reject) => setTimeout(() => reject(new Error('timed out after 10 s')), 10000))]);
			console.log(`ok ${name}`);
		} catch (error) {
			failed++;
			console.log(`FAILED ${name}: ${error.message}`);
		}
		await vscode.commands.executeCommand('workbench.action.closeAllEditors');
		disposables.splice(0).forEach(disposable => disposable.dispose());
	}
	if (failed) {
		throw new Error(`${failed} failed`);
	}
};
