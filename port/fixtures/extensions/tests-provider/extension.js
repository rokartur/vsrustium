// Each `test('name', ...)` line of a *.test.js file is one test; a test whose line says `fail` fails.
const vscode = require('vscode');

exports.activate = async function (context) {
	const controller = vscode.tests.createTestController('portTests', 'Port Tests');
	context.subscriptions.push(controller);
	const files = await vscode.workspace.findFiles('**/*.test.js', '**/node_modules/**');
	console.log(`tests-provider: ${files.length} test files`);
	for (const uri of files) {
		const file = controller.createTestItem(uri.toString(), vscode.workspace.asRelativePath(uri), uri);
		file.canResolveChildren = true;
		controller.items.add(file);
	}
	const failing = new Set();
	controller.resolveHandler = async item => {
		if (!item) {
			return;
		}
		const text = (await vscode.workspace.fs.readFile(item.uri)).toString();
		text.split('\n').forEach((line, index) => {
			const match = /test\('([^']+)'/.exec(line);
			if (match) {
				const test = controller.createTestItem(`${item.id}/${match[1]}`, match[1], item.uri);
				test.range = new vscode.Range(index, 0, index, line.length);
				if (line.includes('fail')) {
					failing.add(test.id);
				}
				item.children.add(test);
			}
		});
	};
	const runHandler = debug => async (request, token) => {
		const run = controller.createTestRun(request);
		const queue = [];
		const collect = item => item.children.size ? item.children.forEach(collect) : queue.push(item);
		(request.include ?? [...controller.items].map(([, item]) => item)).forEach(collect);
		for (const test of queue) {
			if (token.isCancellationRequested) {
				break;
			}
			run.started(test);
			run.appendOutput(`running ${test.label}\r\n`, undefined, test);
			if (failing.has(test.id)) {
				run.failed(test, new vscode.TestMessage(`${test.label} failed`), 5);
			} else {
				run.passed(test, 3);
			}
		}
		if (debug) {
			run.appendOutput('debug profile\r\n');
		}
		run.end();
	};
	controller.createRunProfile('Run', vscode.TestRunProfileKind.Run, runHandler(false), true);
	controller.createRunProfile('Debug', vscode.TestRunProfileKind.Debug, runHandler(true), true);
};
