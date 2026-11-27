// The VS Code tree the port reads (roadmap milestone 22), resolved as native/vscode_src.rs does: $VSCODE_SRC (relative
// to the repo root), else upstream/, else the repo root while the tree is not split.
import { existsSync } from 'node:fs';
import { join, resolve } from 'node:path';

const repo = join(import.meta.dir, '../..');
export const vscodeSrc = process.env.VSCODE_SRC ? resolve(repo, process.env.VSCODE_SRC) : existsSync(join(repo, 'upstream')) ? join(repo, 'upstream') : repo;
