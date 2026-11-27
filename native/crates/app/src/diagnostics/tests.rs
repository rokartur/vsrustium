use std::path::{Path, PathBuf};

use super::*;

fn temp_folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vsrustium-diagnostics-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn workspace_stats_count_types_and_config_files_outside_node_modules_and_git() {
    let root = temp_folder("stats");
    for file in ["a.ts", "b.ts", "c.js", "Makefile", "package.json", ".github/workflows/ci.yml", "node_modules/x.js", ".git/HEAD", ".claude/skills/review/SKILL.md", "docs/.yml"] {
        write(&root, file, "");
    }
    write(&root, ".vscode/launch.json", r#"{ "configurations": [{ "type": "node" }, { "type": "node" }, { "type": "chrome" }], }"#);
    let stats = collect_workspace_stats(&root);
    assert_eq!(stats.file_count, 9);
    assert!(!stats.max_files_reached);
    assert_eq!(stats.file_types.iter().find(|(name, _)| name == "ts"), Some(&("ts".to_owned(), 2)));
    assert_eq!(stats.file_types.iter().find(|(name, _)| name == NO_EXT_KEY).map(|(_, count)| *count), Some(1), "Makefile");
    let mut config: Vec<_> = stats.config_files.iter().map(|(name, _)| name.as_str()).collect();
    config.sort();
    // docs/.yml is no github action (`^.+\.ya?ml$`, and not in .github/workflows).
    assert_eq!(config, ["claude-skills-dir", "github-actions", "launch.json", "makefile", "package.json"]);
    assert_eq!(stats.launch_config_files, [("node".to_owned(), 2), ("chrome".to_owned(), 1)]);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn workspace_stats_wrap_at_sixty_columns_and_fold_the_rest_into_other() {
    let mut file_types: Vec<(String, usize)> = (0..12).map(|index| (format!("ext{index}"), 20 - index)).collect();
    file_types.push((NO_EXT_KEY.into(), 3));
    let stats = WorkspaceStats { file_types, config_files: vec![("package.json".into(), 1)], file_count: 0, max_files_reached: false, launch_config_files: vec![("node".into(), 2), ("chrome".into(), 1)] };
    let text = format_workspace_stats(&stats);
    assert_eq!(
        text,
        "|      File types: ext0(20) ext1(19) ext2(18) ext3(17) ext4(16) ext5(15)\n\
         |                  ext6(14) ext7(13) ext8(12) ext9(11) other(22)\n\
         |      Conf files: package.json(1)\n\
         |      Launch Configs: node(2) chrome"
    );
}

#[test]
fn the_report_names_the_app_the_windows_and_the_hosts() {
    crate::product::load(Path::new(env!("VSCODE_SRC")));
    let host = ProcessItem { name: "bun".into(), cmd: "bun bootstrap-exthost.js".into(), pid: 20, ppid: 10, load: 1.4, mem: 50. * 1024. * 1024., children: Vec::new() };
    let shell = ProcessItem { name: "/bin/zsh -l".into(), cmd: "/bin/zsh -l".into(), pid: 21, ppid: 10, load: 0., mem: 0., children: Vec::new() };
    let root = ProcessItem { name: "vsrustium".into(), cmd: "vsrustium".into(), pid: 10, ppid: 1, load: 2.5, mem: 300. * 1024. * 1024., children: vec![host, shell] };
    let info = MainDiagnostics {
        main_pid: 10,
        main_arguments: vec!["/tmp/w".into()],
        windows: vec![WindowDiagnostics { id: 1, pid: 10, title: "a.ts \u{2014} w".into(), folders: vec![PathBuf::from("/tmp/w")] }],
        pid_to_names: vec![(20, "extension-host [1]".into())],
        screen_reader: false,
    };
    let machine = Machine { os_type: "Darwin".into(), arch: "arm64".into(), release: "25.0.0".into(), cpu_model: "Apple M2".into(), cpu_count: 8, cpu_speed: 0, total_memory: 16. * 1024. * 1024. * 1024., free_memory: 4. * 1024. * 1024. * 1024., load_average: [1.6, 1.2, 0.4], vm_hint: 0. };
    let stats = |_: &Path| WorkspaceStats { file_types: vec![("ts".into(), 2)], file_count: 2, ..WorkspaceStats::default() };
    let report = format_diagnostics(&info, &machine, Some(&root), &stats);
    let lines: Vec<&str> = report.lines().collect();
    assert!(lines[1].starts_with("Version:          VSRustium 1.147"), "{}", lines[1]);
    assert_eq!(&lines[2..10], [
        "OS Version:       Darwin arm64 25.0.0",
        "CPUs:             Apple M2 (8 x 0)",
        "Memory (System):  16.00GB (4.00GB free)",
        "Load (avg):       2, 1, 0",
        "VM:               0%",
        "Screen Reader:    no",
        "Process Argv:     /tmp/w",
        "GPU Status:       ",
    ]);
    assert_eq!(&lines[11..15], ["CPU %\tMem MB\t   PID\tProcess", "    3\t   300\t    10\tvsrustium", "    1\t    50\t    20\textension-host [1]", "    0\t     0\t    21\t   /bin/zsh -l"]);
    assert_eq!(&lines[16..21], ["Workspace Stats: ", "|  Window (a.ts \u{2014} w)", "|    Folder (w): 2 files", "|      File types: ts(2)", "|      Conf files:"]);
}
