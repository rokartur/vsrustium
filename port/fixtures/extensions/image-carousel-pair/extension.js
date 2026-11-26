// Runs the Explorer's "Open in Images Preview" on the workspace folder at startup, for `port/tools/ui-pair` with
// `ARGS=--extensionDevelopmentPath=<this folder>`: the same command as the context menu item, without driving a
// native context menu in the reference.
const vscode = require('vscode');

exports.activate = () => {
	setTimeout(() => vscode.commands.executeCommand('workbench.action.openImagesInCarousel', vscode.workspace.workspaceFolders[0].uri), 1000);
};
