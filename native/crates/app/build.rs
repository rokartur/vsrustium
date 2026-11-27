//! The NLS index: every `localize`/`localize2` call of src/vs with literal key and message, as
//! `module\tkey\tmessage` lines (the message's escapes kept), for the display language (src/nls.rs).

use std::path::Path;

include!("../../vscode_src.rs");

fn main() {
    let src = emit_vscode_src().join("src");
    println!("cargo::rerun-if-changed={}", src.join("vs").display());
    let call = regex::Regex::new(r#"\blocalize2?\(\s*(?:'([^']+)'|"([^"]+)"|\{\s*key:\s*['"]([^'"]+)['"](?:[^{}]|\{[^{}]*\})*\})\s*,\s*(?:"((?:[^"\\\n]|\\.)*)"|'((?:[^'\\\n]|\\.)*)'|`((?:[^`\\]|\\.)*)`)"#).unwrap();
    let mut index = String::new();
    let mut files = Vec::new();
    collect_ts(&src.join("vs"), &mut files);
    files.sort();
    for file in files {
        let module = file.strip_prefix(&src).unwrap().with_extension("").to_string_lossy().into_owned();
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        for captures in call.captures_iter(&text) {
            let key = captures.get(1).or(captures.get(2)).or(captures.get(3)).unwrap().as_str();
            let message = captures.get(4).or(captures.get(5)).or(captures.get(6)).unwrap().as_str();
            if message.contains('\t') || message.contains('\n') {
                continue;
            }
            index.push_str(&format!("{module}\t{key}\t{message}\n"));
        }
    }
    std::fs::write(Path::new(&std::env::var("OUT_DIR").unwrap()).join("nls_index.tsv"), index).unwrap();
}

/// The .ts files of `dir`, without tests and type declarations.
fn collect_ts(dir: &Path, files: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if name != "test" {
                collect_ts(&path, files);
            }
        } else if name.ends_with(".ts") && !name.ends_with(".d.ts") {
            files.push(path);
        }
    }
}
