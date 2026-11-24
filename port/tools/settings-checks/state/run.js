const fs = require('fs');
exports.activate = async context => {
	const g = context.globalState, w = context.workspaceState;
	const env = require('vscode').env;
	const before = { global: g.get('runs'), workspace: w.get('runs'), keys: g.keys(), isNewAppInstall: env.isNewAppInstall };
	await g.update('runs', (g.get('runs') ?? 0) + 1);
	await w.update('runs', (w.get('runs') ?? 0) + 10);
	await g.update('obj', { a: [1, 'two'], b: null });
	g.setKeysForSync(['runs']);
	fs.appendFileSync(process.env.CHECK_OUT, JSON.stringify(before) + '\n');
	fs.appendFileSync(process.env.CHECK_IDS, env.machineId + ' ' + env.sessionId + '\n');
};
