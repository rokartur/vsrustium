// Opens the built-in Simple Browser's own view (its SimpleBrowserManager from the shipped extension) on a local
// page at startup, for `port/tools/ui-pair` with `ARGS=--extensionDevelopmentPath=<this folder>`. The reference's
// `simpleBrowser.show` hands URLs to its integrated browser, which the port removed; calling the manager shows the
// same Simple Browser view in both apps.
const http = require('http');
const path = require('path');
const vscode = require('vscode');

exports.activate = context => {
	const server = http.createServer((_request, response) => {
		response.writeHead(200, { 'Content-Type': 'text/html' });
		response.end('<!DOCTYPE html><title>page</title><body style="font: 16px sans-serif; background: #fff; color: #222"><h1>Simple Browser fixture</h1><p>A local page.</p></body>');
	});
	context.subscriptions.push({ dispose: () => server.close() });
	server.listen(0, '127.0.0.1', () => {
		const extension = vscode.extensions.getExtension('vscode.simple-browser');
		const { SimpleBrowserManager } = require(path.join(extension.extensionPath, 'out', 'simpleBrowserManager.js'));
		const manager = new SimpleBrowserManager(extension.extensionUri);
		context.subscriptions.push(manager);
		manager.show(`http://127.0.0.1:${server.address().port}/`);
	});
};
