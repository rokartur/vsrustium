// Maps the upstream diff between two VS Code tags onto port/ledger.md rows, for planning an uplift.
// Run from the repo root after `git fetch origin tag <to> --no-tags`: bun port/tools/uplift-diff.ts <to> [from]
// `from` defaults to package.json's version, the release the port follows.
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { vscodeSrc } from './vscode-src.ts';

const to = process.argv[2];
if (!to) {
	console.error('usage: bun port/tools/uplift-diff.ts <to-tag> [from-tag]');
	process.exit(2);
}
const from = process.argv[3] ?? JSON.parse(readFileSync(join(vscodeSrc, 'package.json'), 'utf8')).version;

const ledger = new Map<string, string>();
// Evidence may hold `|` (MOCHA_GREP alternations), so only the first three cells are read.
for (const match of readFileSync('port/ledger.md', 'utf8').matchAll(/^\| (\S+) \| [^|]+ \| (\S+) \|/gm)) {
	ledger.set(match[1], match[2]);
}

// Same row ids as port/tools/gen-ledger.ts builds from the tree.
function rowOf(path: string): string | undefined {
	const parts = path.split('/');
	const mainThread = path.match(/^src\/vs\/workbench\/api\/browser\/mainThread(\w+)\.ts$/);
	if (mainThread) {
		return `mainthread/MainThread${mainThread[1]}`;
	}
	if (path.startsWith('src/vs/editor/contrib/')) {
		return `editor/contrib/${parts[4]}`;
	}
	if (path.startsWith('src/vs/workbench/browser/parts/') && parts.length > 6 && parts[5] !== 'media') {
		return `workbench/parts/${parts[5]}`;
	}
	for (const [prefix, id] of [['src/vs/workbench/services/', 'workbench/services'], ['src/vs/workbench/contrib/', 'workbench/contrib'], ['src/vs/platform/', 'platform'], ['src/vs/base/', 'base'], ['src/vs/editor/', 'editor']] as const) {
		if (path.startsWith(prefix) && parts.length > prefix.split('/').length) {
			return `${id}/${parts[prefix.split('/').length - 1]}`;
		}
	}
	if (path.startsWith('extensions/') && parts.length > 2) {
		return `extension/${parts[1]}`;
	}
	return undefined;
}

const isTest = (path: string) => /\/test\/|\.test\.ts$|\.integrationTest\.ts$/.test(path);
// Names that point at the parts Artur removed (AI, sign-in, settings sync, remote, Microsoft services); a hint, not a verdict.
const removedHint = /chat|agent|copilot|mcp|languageModel|inlineChat|\bai|embedding|speech|session|userDataSync|sync|authentication|account|remote|tunnel|telemetry|experiment|survey|issue/i;

const numstat = Bun.spawnSync(['git', 'diff', '--numstat', '-M', from, to, '--', 'src/vs', 'src/vscode-dts', 'extensions', 'product.json'], { cwd: vscodeSrc });
if (numstat.exitCode !== 0) {
	console.error(numstat.stderr.toString() || `git diff ${from} ${to} failed: fetch the tag first`);
	process.exit(1);
}

type Entry = { added: number; deleted: number; files: string[]; tests: number };
const rows = new Map<string, Entry>();
const unmapped = new Map<string, number>();
const dts: string[] = [];
for (const line of numstat.stdout.toString().trim().split('\n').filter(Boolean)) {
	const [added, deleted, rawPath] = line.split('\t');
	const path = rawPath.includes(' => ') ? rawPath.replace(/\{([^}]*) => ([^}]*)\}/, '$2').replace(/^.* => /, '') : rawPath;
	if (path.startsWith('src/vscode-dts/')) {
		dts.push(`${path} +${added}/-${deleted}`);
		continue;
	}
	const id = rowOf(path);
	const parts = path.split('/');
	if (!id) {
		// Workbench shell files (layout.ts, api/common/extHost*.ts, common/*.ts) touch many rows: listed one by one.
		const folder = path.startsWith('src/vs/workbench/') && !isTest(path) ? path : parts.slice(0, path.startsWith('src/') ? 3 : 1).join('/');
		unmapped.set(folder, (unmapped.get(folder) ?? 0) + 1);
		continue;
	}
	const entry = rows.get(id) ?? { added: 0, deleted: 0, files: [], tests: 0 };
	if (isTest(path)) {
		entry.tests += 1;
	} else {
		entry.added += Number(added) || 0;
		entry.deleted += Number(deleted) || 0;
		entry.files.push(path);
	}
	rows.set(id, entry);
}

const groups: Record<string, string[]> = { 'to port (row not removed)': [], 'new upstream unit, no ledger row': [], 'removed by decision (skip)': [], 'tests only': [] };
for (const [id, entry] of [...rows].sort((a, b) => b[1].added + b[1].deleted - (a[1].added + a[1].deleted))) {
	const status = ledger.get(id);
	const size = `+${entry.added}/-${entry.deleted}, ${entry.files.length} files${entry.tests ? `, ${entry.tests} test files` : ''}`;
	if (entry.files.length === 0) {
		groups['tests only'].push(`- ${id} (${status ?? 'no row'}): ${entry.tests} test files`);
	} else if (status === 'removed' || status === 'n/a') {
		groups['removed by decision (skip)'].push(`- ${id} (${status}): ${size}`);
	} else if (!status) {
		groups['new upstream unit, no ledger row'].push(`- ${id}: ${size}${removedHint.test(id) ? ' (name suggests a removed area: check port/notes.md decisions)' : ''}\n${entry.files.map(file => `  - ${file}`).join('\n')}`);
	} else {
		groups['to port (row not removed)'].push(`- ${id} (${status}): ${size}\n${entry.files.map(file => `  - ${file}`).join('\n')}`);
	}
}

console.log(`# Upstream diff ${from} to ${to}, mapped to port/ledger.md\n`);
for (const [title, lines] of Object.entries(groups)) {
	console.log(`## ${title} (${lines.length})\n\n${lines.join('\n') || '- none'}\n`);
}
console.log(`## API typings (src/vscode-dts) (${dts.length})\n\n${dts.map(line => `- ${line}`).join('\n') || '- none'}\n`);
console.log(`## Not mapped to a row, files per folder (${unmapped.size})\n\n${[...unmapped].map(([folder, count]) => `- ${folder}: ${count}`).join('\n') || '- none'}`);
