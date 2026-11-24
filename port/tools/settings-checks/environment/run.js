const fs = require('fs');
const vscode = require('vscode');
let context;
exports.activate = c => { context = c; };
exports.run = async () => {
	const env = vscode.env;
	const strip = value => String(value).split(process.env.CHECK_DIR).join('<dir>');
	const out = {
		appName: env.appName, appRoot: env.appRoot ? 'set' : env.appRoot, appHost: env.appHost, uriScheme: env.uriScheme, language: env.language,
		machineId: /^[0-9a-f-]{36}$/.test(env.machineId), sessionId: /^[0-9a-f-]{36}\d+$/.test(env.sessionId), isNewAppInstall: env.isNewAppInstall,
		isTelemetryEnabled: env.isTelemetryEnabled, remoteName: env.remoteName, uiKind: env.uiKind, logLevel: env.logLevel,
		extensionMode: context.extensionMode, globalStorage: strip(context.globalStorageUri.fsPath), storage: strip(context.storageUri && context.storageUri.fsPath).replace(/[0-9a-f]{32}/, '<id>'),
		logUri: strip(context.logUri.fsPath).replace(/\d{8}T\d{6}/, '<time>'), workspaceName: vscode.workspace.name, version: vscode.version,
		extensionKind: context.extension.extensionKind, extensionPath: strip(context.extensionPath),
	};
	fs.writeFileSync(process.env.CHECK_OUT, JSON.stringify(out, null, 1));
};
