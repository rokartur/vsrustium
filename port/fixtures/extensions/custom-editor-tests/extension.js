// The custom text editor of extensions/vscode-custom-editor-tests/src/customTextEditor.ts, which upstream removed in
// bbe7b54cf4d; port/tools/webview-check runs its tests (tests/index.js).
const path = require('path');
const vscode = require('vscode');

const contentChangeCommand = '_abcEditor.contentChange';
const typeCommand = '_abcEditor.type';

exports.activate = context => {
	let activeEditor;
	context.subscriptions.push(
		vscode.window.registerCustomEditorProvider('testWebviewEditor.abc', {
			resolveCustomTextEditor(document, panel) {
				const editor = createEditor(document, context.extensionPath, panel);
				activeEditor = editor;
				panel.onDidChangeViewState(({ webviewPanel }) => {
					if (activeEditor === editor && !webviewPanel.active) {
						activeEditor = undefined;
					}
					if (webviewPanel.active) {
						activeEditor = editor;
					}
				});
			},
		}),
		vscode.commands.registerCommand(typeCommand, content => activeEditor?.fakeInput(content)),
	);
};

function createEditor(document, extensionPath, panel) {
	const subscriptions = [];
	let disposed = false;
	let syncedVersion = -1;
	let currentWorkspaceEdit;
	// p-limit(1) upstream: one workspace edit at a time.
	let editQueue = Promise.resolve();

	const update = async () => {
		await currentWorkspaceEdit;
		if (disposed || syncedVersion >= document.version) {
			return;
		}
		panel.webview.postMessage({ type: 'setValue', value: document.getText() });
		syncedVersion = document.version;
	};

	const doEdit = value => {
		const edit = new vscode.WorkspaceEdit();
		edit.replace(document.uri, document.validateRange(new vscode.Range(0, 0, 999999, 999999)), value);
		editQueue = editQueue.then(() => {
			currentWorkspaceEdit = vscode.workspace.applyEdit(edit).then(() => {
				syncedVersion = document.version;
				currentWorkspaceEdit = undefined;
			});
			return currentWorkspaceEdit;
		});
	};

	panel.webview.options = { enableScripts: true };
	const nonce = Math.random().toString(36).slice(2);
	const script = panel.webview.asWebviewUri(vscode.Uri.file(path.join(extensionPath, 'media', 'textEditor.js')));
	panel.webview.html = `<!DOCTYPE html>
		<html lang="en">
		<head>
			<meta charset="UTF-8">
			<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'nonce-${nonce}'; style-src 'unsafe-inline';">
			<title>Document</title>
		</head>
		<body>
			<textarea style="width: 300px; height: 300px;"></textarea>
			<script nonce="${nonce}" src="${script}"></script>
		</body>
		</html>`;

	subscriptions.push(vscode.workspace.onDidChangeTextDocument(e => {
		if (e.document === document) {
			update();
		}
	}));
	subscriptions.push(panel.webview.onDidReceiveMessage(message => {
		switch (message.type) {
			case 'edit':
				doEdit(message.value);
				break;
			case 'didChangeContent':
				vscode.commands.executeCommand(contentChangeCommand, { content: message.value, source: document.uri });
				break;
		}
	}));
	subscriptions.push(panel.onDidDispose(() => {
		disposed = true;
		subscriptions.forEach(subscription => subscription.dispose());
	}));
	update();

	return { fakeInput: value => panel.webview.postMessage({ type: 'fakeInput', value }) };
}
