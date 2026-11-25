// The files a VSRustium build loads at runtime: the native binary, the extension host bundle and the built-in
// extensions with every package they resolve, transitively. Shared by port/tools/telemetry-source-check and the
// app packaging (port/tools/package-app), so the check covers exactly what ships.
import { existsSync, readdirSync, readFileSync, realpathSync, statSync } from 'node:fs';
import { dirname, join } from 'node:path';

// native/crates/exthost/src/scan.rs REMOVED_BUILTINS and TEST_BUILTINS, read from the source so they cannot drift.
const scanSource = readFileSync(join(import.meta.dir, '../../native/crates/exthost/src/scan.rs'), 'utf8');
const skippedBuiltins = [...scanSource.matchAll(/(?:REMOVED|TEST)_BUILTINS: \[&str; \d+\] = \[([^\]]*)\]/g)].flatMap(match => [...match[1].matchAll(/"([^"]+)"/g)].map(name => name[1]));

// Served as inert modules by the extension host (src/vs/workbench/api/common/extHostInertModules.ts): never loaded.
const inertSource = readFileSync(join(import.meta.dir, '../../src/vs/workbench/api/common/extHostInertModules.ts'), 'utf8');
export const inertPackages = [...inertSource.matchAll(/^\t\['([^']+)', \{/gm)].map(match => match[1]);

// Not loaded at runtime: sources, maps, docs and tests.
const skippedFile = /\.(ts|map|md|markdown)$|^(LICENSE|CHANGELOG|README)/i;
const skippedFolder = new Set(['test', 'tests', '__tests__', 'node_modules', '.vscode', '.github']);

export function shippedBuiltins(extensionsDir: string): string[] {
	return readdirSync(extensionsDir)
		.filter(name => existsSync(join(extensionsDir, name, 'package.json')) && !skippedBuiltins.includes(name))
		.map(name => join(extensionsDir, name))
		.sort();
}

/** Package folders (real paths) each built-in extension resolves through its dependencies, the inert ones left out. */
export function shippedPackages(extensionDirs: string[]): Set<string> {
	const packages = new Set<string>();
	const visit = (dir: string) => {
		const manifest = JSON.parse(readFileSync(join(dir, 'package.json'), 'utf8'));
		const dependencies = { ...manifest.dependencies, ...manifest.optionalDependencies };
		for (const name of Object.keys(dependencies)) {
			if (inertPackages.includes(name)) {
				continue;
			}
			const found = resolvePackage(name, dir);
			if (found && !packages.has(found)) {
				packages.add(found);
				visit(found);
			}
		}
	};
	extensionDirs.forEach(visit);
	return packages;
}

// Node's lookup: <dir>/node_modules/<name> in the folder and each parent, from the real path (pnpm links).
function resolvePackage(name: string, from: string): string | undefined {
	for (let dir = realpathSync(from); ; dir = dirname(dir)) {
		const candidate = join(dir, 'node_modules', name);
		if (existsSync(join(candidate, 'package.json'))) {
			return realpathSync(candidate);
		}
		if (dir === dirname(dir)) {
			return undefined;
		}
	}
}

/** Every runtime file under `dir`, without nested node_modules (each package is listed on its own). */
export function runtimeFiles(dir: string): string[] {
	const files: string[] = [];
	for (const entry of readdirSync(dir, { withFileTypes: true })) {
		const path = join(dir, entry.name);
		if (entry.isDirectory() || (entry.isSymbolicLink() && statSync(path).isDirectory())) {
			if (!skippedFolder.has(entry.name)) {
				files.push(...runtimeFiles(path));
			}
		} else if (!skippedFile.test(entry.name)) {
			files.push(path);
		}
	}
	return files;
}
