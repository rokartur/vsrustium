// Each command shows one kind of message; PORT_MESSAGES (comma separated command suffixes) runs them at startup.
const vscode = require('vscode');

const shows = {
	info: () => vscode.window.showInformationMessage('Information message from an extension', 'Yes', 'No'),
	warning: () => vscode.window.showWarningMessage('Warning message without actions'),
	error: () => vscode.window.showErrorMessage('Error message with an action', 'Retry'),
	long: () => vscode.window.showInformationMessage('A long message that does not fit on one line of the toast, so the toast shows it cut with an ellipsis until it is expanded by a click on the chevron'),
	modal: () => vscode.window.showWarningMessage('Modal message', { modal: true, detail: 'Detail of the modal message' }, 'Save', 'Discard'),
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
