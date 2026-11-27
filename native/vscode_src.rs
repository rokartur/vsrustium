// The VS Code tree the port reads (roadmap milestone 22): $VSCODE_SRC, else upstream/ of the repo, else the repo root
// while the tree is not split. Included by the build scripts of app, diff and files, which export it as
// env!("VSCODE_SRC"); port/tools/vscode-src.sh and port/tools/vscode-src.ts resolve it the same way.
fn emit_vscode_src() -> std::path::PathBuf {
    println!("cargo::rerun-if-env-changed=VSCODE_SRC");
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let src = match std::env::var_os("VSCODE_SRC") {
        Some(path) => repo.join(path),
        None if repo.join("upstream").is_dir() => repo.join("upstream"),
        None => repo,
    };
    let src = src.canonicalize().unwrap_or_else(|error| panic!("VSCODE_SRC {}: {error}", src.display()));
    println!("cargo::rustc-env=VSCODE_SRC={}", src.display());
    src
}
