const vscode = require('vscode');

exports.activate = function (context) {
	const item = (name, kind, uri, line) => {
		const range = new vscode.Range(line, 0, line, 20);
		return new vscode.TypeHierarchyItem(kind, name, 'a.ts', uri, range, range);
	};
	context.subscriptions.push(vscode.languages.registerTypeHierarchyProvider({ language: 'typescript' }, {
		prepareTypeHierarchy(document, position) {
			const range = document.getWordRangeAtPosition(position);
			return range ? item(document.getText(range), vscode.SymbolKind.Class, document.uri, range.start.line) : undefined;
		},
		provideTypeHierarchySupertypes(target) {
			if (target.name !== 'add') {
				return [];
			}
			return [item('Base', vscode.SymbolKind.Class, target.uri, 3), item('Shape', vscode.SymbolKind.Interface, target.uri, 4)];
		},
		provideTypeHierarchySubtypes() {
			return [];
		},
	}));
};
