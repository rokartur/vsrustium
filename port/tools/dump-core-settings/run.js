// Extension test runner for port/tools/dump-core-settings: writes the reference's settings registry as the
// Settings editor sees it: vscode://schemas/settings/default with its $defs inlined.
const vscode = require('vscode');
const fs = require('fs');
const path = require('path');

exports.activate = () => { };

exports.run = async () => {
	const read = async uri => new TextDecoder().decode(await vscode.workspace.fs.readFile(vscode.Uri.parse(uri)));
	// The workbench registers the schema once it has restored; until then the file does not exist.
	let schema;
	for (let attempt = 0; !schema; attempt++) {
		schema = await read('vscode://schemas/settings/default').then(JSON.parse, error => {
			if (attempt === 120) {
				throw error;
			}
			return new Promise(resolve => setTimeout(resolve, 500));
		});
	}
	const resolve = node => {
		if (Array.isArray(node)) {
			return node.map(resolve);
		}
		if (!node || typeof node !== 'object') {
			return node;
		}
		if (typeof node.$ref === 'string' && node.$ref.startsWith('#/$defs/')) {
			const { $ref, ...rest } = node;
			return resolve({ ...schema.$defs[$ref.slice('#/$defs/'.length)], ...rest });
		}
		return Object.fromEntries(Object.entries(node).map(([key, value]) => [key, resolve(value)]));
	};
	// Built-in extensions stay enabled under --disable-extensions: drop their settings and the defaults they override
	// (the port registers them from each package.json); language defaults of core come from modesRegistry.ts in the port.
	const core = {};
	for (const [key, property] of Object.entries(resolve(schema.properties))) {
		if (property.section?.extensionInfo || key.startsWith('[')) {
			continue;
		}
		const { defaultDefaultValue, defaultValueSource, ...rest } = property;
		core[key] = { ...rest, default: defaultDefaultValue !== undefined ? defaultDefaultValue : defaultOfType(property.type) };
	}
	fs.writeFileSync(path.join(process.env.DUMP_OUT, 'core-settings.json'), JSON.stringify(core, null, '\t') + '\n');
	// The schemas the settings schemas $ref, as settingsFilesystemProvider.ts serves them, the package.json schema of
	// extensionsRegistry.ts that extension-editing's jsonValidation names, and the snippets and keybindings file schemas.
	fs.mkdirSync(path.join(process.env.DUMP_OUT, 'schemas'));
	for (const name of ['workbench-colors', 'token-styling', 'textmate-colors', 'size-customizations', 'launch', 'vscode-extensions', 'toolsParameters', 'snippets', 'global-snippets', 'keybindings']) {
		fs.writeFileSync(path.join(process.env.DUMP_OUT, 'schemas', `${name}.json`), await read(`vscode://schemas/${name}`));
	}
};

// getDefaultValue of configurationRegistry.ts.
function defaultOfType(type) {
	switch (Array.isArray(type) ? type[0] : type) {
		case 'boolean': return false;
		case 'integer': case 'number': return 0;
		case 'string': return '';
		case 'array': return [];
		case 'object': return {};
		default: return null;
	}
};
