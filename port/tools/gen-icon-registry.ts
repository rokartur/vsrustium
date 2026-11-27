// Emits native/crates/app/src/icon_registry.json from the upstream sources: each registered icon id with the id of
// its default (iconRegistry.ts `IconDefaults` as a ThemeIcon), for registerIcon calls and codicons registered over
// another codicon (codicons.ts `register('dialog-error', 'error')`). Ids with a font character are not listed.
import { Glob } from 'bun';
import { join } from 'node:path';
import { vscodeSrc } from './vscode-src.ts';

const codiconIds = new Map<string, string>();
const defaults: Record<string, string> = {};
for (const file of ['src/vs/base/common/codiconsLibrary.ts', 'src/vs/base/common/codicons.ts']) {
	const source = await Bun.file(join(vscodeSrc, file)).text();
	for (const [, name, id, , alias] of source.matchAll(/(\w+): register\('([\w-]+)', (0x[0-9a-fA-F]+|'([\w-]+)')\)/g)) {
		codiconIds.set(name, id);
		if (alias) {
			defaults[id] = alias;
		}
	}
}

const calls: { id: string; target: string }[] = [];
const variables = new Map<string, string>();
for await (const file of new Glob('src/vs/**/*.ts').scan(vscodeSrc)) {
	if (file.includes('/test/')) {
		continue;
	}
	const source = await Bun.file(join(vscodeSrc, file)).text();
	for (const [, variable, id, target] of source.matchAll(/(?:const (\w+)\s*=\s*)?registerIcon\(\s*'([\w-]+)',\s*([\w.]+)/g)) {
		calls.push({ id, target });
		if (variable) {
			variables.set(variable, id);
		}
	}
}
let skipped = 0;
for (const { id, target } of calls) {
	const codicon = target.startsWith('Codicon.') ? codiconIds.get(target.slice('Codicon.'.length)) : variables.get(target);
	// registerIcon over a codicon's own id returns the codicon (iconRegistry.ts `registerIcon` keeps an existing id).
	if (codicon === id) {
		continue;
	}
	if (codicon) {
		defaults[id] = codicon;
	} else {
		skipped++;
	}
}
const sorted = Object.fromEntries(Object.entries(defaults).sort(([a], [b]) => a.localeCompare(b)));
await Bun.write('native/crates/app/src/icon_registry.json', JSON.stringify(sorted, null, '\t') + '\n');
console.log(`${Object.keys(sorted).length} icons, ${skipped} registerIcon calls without a resolvable default`);
