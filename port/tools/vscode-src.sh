# Sourced by the tools: exports VSCODE_SRC, the VS Code tree the port reads (roadmap milestone 22), resolved as
# native/vscode_src.rs does: $VSCODE_SRC (relative to the repo root), else upstream/, else the repo root while the
# tree is not split.
if [[ -n ${VSCODE_SRC:-} ]]; then
  VSCODE_SRC=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && cd "$VSCODE_SRC" && pwd)
elif [[ -d $(dirname "${BASH_SOURCE[0]}")/../../upstream ]]; then
  VSCODE_SRC=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../upstream" && pwd)
else
  VSCODE_SRC=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
fi
export VSCODE_SRC
