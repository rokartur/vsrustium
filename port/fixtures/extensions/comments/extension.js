const vscode = require('vscode');

exports.activate = async function (context) {
	const controller = vscode.comments.createCommentController('port-comments', 'Port Comments');
	controller.reactionHandler = async (comment, reaction) => {
		const own = comment.reactions.find(candidate => candidate.label === reaction.label);
		own.authorHasReacted = !own.authorHasReacted;
		own.count += own.authorHasReacted ? 1 : -1;
		refresh(comment);
	};
	const threadOf = new Map();
	const refresh = comment => {
		const thread = threadOf.get(comment);
		thread.comments = [...thread.comments];
	};
	controller.commentingRangeProvider = {
		provideCommentingRanges: document => [new vscode.Range(0, 0, document.lineCount - 1, 0)],
	};
	context.subscriptions.push(controller);
	const folder = vscode.workspace.workspaceFolders?.[0];
	if (!folder) {
		return;
	}
	const uri = vscode.Uri.joinPath(folder.uri, 'a.ts');
	const author = name => ({ name });
	const first = controller.createCommentThread(uri, new vscode.Range(1, 0, 1, 10), [
		{ body: 'Should this return a number?', author: author('Ann'), mode: vscode.CommentMode.Preview, contextValue: 'editable', reactions: [{ label: '+1', iconPath: '', count: 2, authorHasReacted: true }] },
		{ body: new vscode.MarkdownString('It does, see **add**.'), author: author('Bob'), mode: vscode.CommentMode.Preview },
	]);
	first.state = vscode.CommentThreadState.Unresolved;
	first.comments.forEach(comment => threadOf.set(comment, first));
	controller.createCommentThread(uri, new vscode.Range(3, 0, 4, 5), [
		{ body: 'Rename total', author: author('Cid'), mode: vscode.CommentMode.Preview },
	]);
	context.subscriptions.push(
		vscode.commands.registerCommand('port-comments.reply', reply => {
			const comment = { body: reply.text, author: author('Dee'), mode: vscode.CommentMode.Preview };
			reply.thread.comments = [...reply.thread.comments, comment];
		}),
		vscode.commands.registerCommand('port-comments.resolve', thread => {
			thread.state = vscode.CommentThreadState.Resolved;
		}),
		vscode.commands.registerCommand('port-comments.edit', comment => {
			comment.mode = vscode.CommentMode.Editing;
			refresh(comment);
		}),
		vscode.commands.registerCommand('port-comments.save', comment => {
			comment.body = comment.body + ' (edited)';
			comment.mode = vscode.CommentMode.Preview;
			refresh(comment);
		}),
	);
	await vscode.commands.executeCommand('workbench.action.focusCommentsPanel');
};
