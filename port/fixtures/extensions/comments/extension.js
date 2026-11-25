const vscode = require('vscode');

exports.activate = async function (context) {
	const controller = vscode.comments.createCommentController('port-comments', 'Port Comments');
	context.subscriptions.push(controller);
	const folder = vscode.workspace.workspaceFolders?.[0];
	if (!folder) {
		return;
	}
	const uri = vscode.Uri.joinPath(folder.uri, 'a.ts');
	const author = name => ({ name });
	const first = controller.createCommentThread(uri, new vscode.Range(1, 0, 1, 10), [
		{ body: 'Should this return a number?', author: author('Ann'), mode: vscode.CommentMode.Preview },
		{ body: new vscode.MarkdownString('It does, see **add**.'), author: author('Bob'), mode: vscode.CommentMode.Preview },
	]);
	first.state = vscode.CommentThreadState.Unresolved;
	controller.createCommentThread(uri, new vscode.Range(3, 0, 4, 5), [
		{ body: 'Rename total', author: author('Cid'), mode: vscode.CommentMode.Preview },
	]);
	await vscode.commands.executeCommand('workbench.action.focusCommentsPanel');
};
