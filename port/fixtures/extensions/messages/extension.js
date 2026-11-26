// Each command shows one kind of message; PORT_MESSAGES (comma separated command suffixes) runs them at startup.
const vscode = require('vscode');

const shows = {
	info: () => vscode.window.showInformationMessage('Information message from an extension', 'Yes', 'No'),
	warning: () => vscode.window.showWarningMessage('Warning message without actions'),
	error: () => vscode.window.showErrorMessage('Error message with an action', 'Retry'),
	long: () => vscode.window.showInformationMessage('A long message that does not fit on one line of the toast, so the toast shows it cut with an ellipsis until it is expanded by a click on the chevron'),
	modal: () => vscode.window.showWarningMessage('Modal message', { modal: true, detail: 'Detail of the modal message' }, 'Save', 'Discard'),
	progress: () => vscode.window.withProgress({ location: vscode.ProgressLocation.Notification, title: 'Notification progress', cancellable: true }, async progress => {
		progress.report({ increment: 40, message: 'step 2 of 5' });
		await new Promise(resolve => setTimeout(resolve, 300000));
	}),
	windowProgress: () => vscode.window.withProgress({ location: vscode.ProgressLocation.Window, title: 'Window progress' }, () => new Promise(resolve => setTimeout(resolve, 300000))),
	status: async () => {
		const left = vscode.window.createStatusBarItem('portMessages.left', vscode.StatusBarAlignment.Left, 5);
		left.text = '$(bell) Port left';
		left.tooltip = 'Left entry from an extension';
		left.command = 'portMessages.info';
		left.show();
		const right = vscode.window.createStatusBarItem('portMessages.right', vscode.StatusBarAlignment.Right, 1000);
		right.text = 'Port warning';
		right.backgroundColor = new vscode.ThemeColor('statusBarItem.warningBackground');
		right.show();
	},
	openDialog: () => vscode.window.showOpenDialog({ canSelectMany: true, openLabel: 'Pick Files', filters: { 'JSON': ['json'] } }),
	saveDialog: () => vscode.window.showSaveDialog({ saveLabel: 'Store', title: 'Save from an extension' }),
	decorations: async () => {
		vscode.window.registerFileDecorationProvider({
			provideFileDecoration: uri => uri.path.endsWith('.json') ? { badge: 'P', tooltip: 'Port decoration', color: new vscode.ThemeColor('charts.red'), propagate: true } : undefined,
		});
	},
};

exports.activate = function (context) {
	for (const [name, show] of Object.entries(shows)) {
		context.subscriptions.push(vscode.commands.registerCommand(`portMessages.${name}`, async () => {
			const picked = await show();
			console.log(`portMessages.${name}: ${picked}`);
		}));
	}
	for (const name of (process.env.PORT_MESSAGES ?? '').split(',').filter(name => name)) {
		shows[name]().then(picked => console.log(`portMessages.${name}: ${picked}`));
	}
};
