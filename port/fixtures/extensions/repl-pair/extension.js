// An echo kernel for the interactive window and REPL editor pairs of `port/tools/ui-pair` with
// `ARGS=--extensionDevelopmentPath=<this folder>`: the user setting `replPair.open` ("interactive" or "repl") opens one
// at startup, with the kernel bound, so both apps show the same editor without a kernel extension.
const vscode = require('vscode');

exports.activate = context => {
	const controllers = {};
	for (const type of ['interactive', 'jupyter-notebook']) {
		const controller = vscode.notebooks.createNotebookController(`echo-${type}`, type, 'Echo Kernel');
		controller.supportedLanguages = ['plaintext'];
		let order = 0;
		controller.executeHandler = cells => {
			for (const cell of cells) {
				const execution = controller.createNotebookCellExecution(cell);
				execution.executionOrder = ++order;
				execution.start(Date.now());
				execution.replaceOutput([new vscode.NotebookCellOutput([vscode.NotebookCellOutputItem.text(`echo: ${cell.document.getText()}`)])]);
				execution.end(true, Date.now());
			}
		};
		controllers[type] = controller;
		context.subscriptions.push(controller);
	}
	const open = vscode.workspace.getConfiguration('replPair').get('open');
	setTimeout(async () => {
		if (open === 'interactive') {
			await vscode.commands.executeCommand('interactive.open', { viewColumn: vscode.ViewColumn.Active, preserveFocus: false }, undefined, 'port.repl-pair/echo-interactive', 'Echo');
		} else if (open === 'repl') {
			const data = new vscode.NotebookData([]);
			const notebook = await vscode.workspace.openNotebookDocument('jupyter-notebook', data);
			controllers['jupyter-notebook'].updateNotebookAffinity(notebook, vscode.NotebookControllerAffinity.Preferred);
			await vscode.window.showNotebookDocument(notebook, { asRepl: 'Echo REPL' });
		}
	}, 1000);
};
