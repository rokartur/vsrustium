// Editor inset checks (the proposed `editorInsets` API, mainThreadCodeInsets.ts) run by
// `FIXTURE=editor-inset-tests port/tools/webview-check`.
const assert = require('assert');
const vscode = require('vscode');

const readme = () => vscode.Uri.joinPath(vscode.workspace.workspaceFolders[0].uri, 'README.md');
const openReadme = async () => vscode.window.showTextDocument(await vscode.workspace.openTextDocument(readme()));

const echoHtml = `<!DOCTYPE html><html><body><script>
	const vscode = acquireVsCodeApi();
	window.addEventListener('message', e => vscode.postMessage({ echo: e.data }));
	vscode.postMessage({ type: 'ready' });
</script></body></html>`;

const nextMessage = inset => new Promise(resolve => {
	const sub = inset.webview.onDidReceiveMessage(message => {
		sub.dispose();
		resolve(message);
	});
});

const disposed = inset => new Promise(resolve => inset.onDidDispose(resolve));

function timeout(what, promise) {
	return Promise.race([promise, new Promise((_, reject) => setTimeout(() => reject(new Error(`timed out waiting for ${what}`)), 15000))]);
}

// The proposed API, from the extension's own `vscode` (see extension.js).
const proposed = () => vscode.extensions.getExtension('port.editor-inset-tests').activate();

const checks = {
	async 'an inset loads its html and exchanges messages'() {
		const editor = await openReadme();
		const inset = (await proposed()).window.createWebviewTextEditorInset(editor, 0, 4, { enableScripts: true });
		const ready = nextMessage(inset);
		inset.webview.html = echoHtml;
		assert.deepStrictEqual(await timeout('ready', ready), { type: 'ready' });
		const reply = nextMessage(inset);
		assert.strictEqual(await inset.webview.postMessage({ value: 42 }), true);
		assert.deepStrictEqual(await timeout('the echo', reply), { echo: { value: 42 } });
		const gone = disposed(inset);
		inset.dispose();
		await timeout('onDidDispose', gone);
	},

	async 'closing the editor disposes its inset'() {
		const editor = await openReadme();
		const inset = (await proposed()).window.createWebviewTextEditorInset(editor, 1, 2);
		const gone = disposed(inset);
		await vscode.commands.executeCommand('workbench.action.closeAllEditors');
		await timeout('onDidDispose', gone);
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
		throw new Error(`${failed} editor inset checks failed`);
	}
};
