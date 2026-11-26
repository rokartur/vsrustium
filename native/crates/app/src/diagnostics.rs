//! platform/diagnostics: `code --status` (main.ts, diagnosticsMainService.ts `getMainDiagnostics`, node/diagnosticsService.ts
//! `getDiagnostics`). The running instance answers its main process facts over the launch socket; the second instance
//! lists the processes, counts the workspace files and prints the report.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::ps::ProcessItem;

/// `IMainProcessDiagnostics` without the GPU fields (no Chromium GPU process in the port).
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MainDiagnostics {
    pub main_pid: u32,
    pub main_arguments: Vec<String>,
    pub windows: Vec<WindowDiagnostics>,
    pub pid_to_names: Vec<(u32, String)>,
    pub screen_reader: bool,
}

/// `IWindowDiagnostics`: every port window runs in the main process, so its pid is the main pid.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WindowDiagnostics {
    pub id: u64,
    pub pid: u32,
    pub title: String,
    pub folders: Vec<std::path::PathBuf>,
}

/// `WorkspaceStats`: counts sorted by count, highest first.
#[derive(Debug, Default, PartialEq)]
pub struct WorkspaceStats {
    pub file_types: Vec<(String, usize)>,
    pub config_files: Vec<(String, usize)>,
    pub file_count: usize,
    pub max_files_reached: bool,
    pub launch_config_files: Vec<(String, usize)>,
}

/// The no-extension sentinel of `collectWorkspaceStats`.
const NO_EXT_KEY: &str = "\0no-extension";
const MAX_FILES: usize = 20000;

/// `collectWorkspaceStats` `configFilePatterns`: tag, file name test, relative folder test.
type NameTest = fn(&str) -> bool;
const CONFIG_FILE_PATTERNS: &[(&str, NameTest, Option<NameTest>)] = &[
    ("grunt.js", |name| name.eq_ignore_ascii_case("gruntfile.js"), None),
    ("gulp.js", |name| name.eq_ignore_ascii_case("gulpfile.js"), None),
    ("tsconfig.json", |name| name.eq_ignore_ascii_case("tsconfig.json"), None),
    ("package.json", |name| name.eq_ignore_ascii_case("package.json"), None),
    ("jsconfig.json", |name| name.eq_ignore_ascii_case("jsconfig.json"), None),
    ("tslint.json", |name| name.eq_ignore_ascii_case("tslint.json"), None),
    ("eslint.json", |name| name.eq_ignore_ascii_case("eslint.json"), None),
    ("tasks.json", |name| name.eq_ignore_ascii_case("tasks.json"), None),
    ("launch.json", |name| name.eq_ignore_ascii_case("launch.json"), None),
    ("mcp.json", |name| name.eq_ignore_ascii_case("mcp.json"), None),
    ("settings.json", |name| name.eq_ignore_ascii_case("settings.json"), None),
    ("webpack.config.js", |name| name.eq_ignore_ascii_case("webpack.config.js"), None),
    ("project.json", |name| name.eq_ignore_ascii_case("project.json"), None),
    ("makefile", |name| name.eq_ignore_ascii_case("makefile"), None),
    ("sln", |name| has_suffix(name, ".sln"), None),
    ("csproj", |name| has_suffix(name, ".csproj"), None),
    ("cmake", |name| has_suffix(name, ".cmake"), None),
    ("github-actions", |name| has_suffix(name, ".yml") || has_suffix(name, ".yaml"), Some(|dir| is_dir(dir, ".github/workflows"))),
    ("devcontainer.json", |name| name.eq_ignore_ascii_case("devcontainer.json"), None),
    ("dockerfile", |name| ["dockerfile", "docker-compose.yml", "docker-compose.yaml"].iter().any(|file| name.eq_ignore_ascii_case(file)), None),
    ("cursorrules", |name| name.eq_ignore_ascii_case(".cursorrules"), None),
    ("cursorrules-dir", |name| ends_with_ignore_case(name, ".mdc"), Some(|dir| is_dir(dir, ".cursor/rules"))),
    ("github-instructions-dir", |name| has_suffix(name, ".instructions.md"), Some(|dir| is_dir(dir, ".github/instructions"))),
    ("github-prompts-dir", |name| has_suffix(name, ".prompt.md"), Some(|dir| is_dir(dir, ".github/prompts"))),
    ("clinerules", |name| name.eq_ignore_ascii_case(".clinerules"), None),
    ("clinerules-dir", |name| ends_with_ignore_case(name, ".md"), Some(|dir| dir.eq_ignore_ascii_case(".clinerules"))),
    ("agent.md", |name| name.eq_ignore_ascii_case("agent.md"), None),
    ("agents.md", |name| name.eq_ignore_ascii_case("agents.md"), None),
    ("claude.md", |name| name.eq_ignore_ascii_case("claude.md"), None),
    ("claude-settings", |name| name.eq_ignore_ascii_case("settings.json"), Some(|dir| dir.eq_ignore_ascii_case(".claude"))),
    ("claude-settings-local", |name| name.eq_ignore_ascii_case("settings.local.json"), Some(|dir| dir.eq_ignore_ascii_case(".claude"))),
    ("claude-mcp", |name| name.eq_ignore_ascii_case("mcp.json"), Some(|dir| dir.eq_ignore_ascii_case(".claude"))),
    ("claude-commands-dir", |name| ends_with_ignore_case(name, ".md"), Some(|dir| is_dir(dir, ".claude/commands"))),
    // `/^\.claude[\/\\]skills[\/\\]/i`: any folder below .claude/skills.
    ("claude-skills-dir", |name| name.eq_ignore_ascii_case("SKILL.md"), Some(|dir| dir.len() > 15 && is_dir(&dir[..15], ".claude/skills/"))),
    ("claude-rules-dir", |name| ends_with_ignore_case(name, ".md"), Some(|dir| is_dir(dir, ".claude/rules"))),
    ("gemini.md", |name| name.eq_ignore_ascii_case("gemini.md"), None),
    ("copilot-instructions.md", |name| name.eq_ignore_ascii_case("copilot-instructions.md"), Some(|dir| dir.eq_ignore_ascii_case(".github"))),
];

/// `/^.+\.ext$/i`: at least one character before the suffix.
fn has_suffix(name: &str, suffix: &str) -> bool {
    name.len() > suffix.len() && ends_with_ignore_case(name, suffix)
}

fn ends_with_ignore_case(name: &str, suffix: &str) -> bool {
    name.len() >= suffix.len() && name.is_char_boundary(name.len() - suffix.len()) && name[name.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
}

/// A relative folder path compared with `/` and `\` alike, as the patterns' `[\/\\]`.
fn is_dir(dir: &str, expected: &str) -> bool {
    dir.len() == expected.len() && dir.chars().zip(expected.chars()).all(|(a, b)| a.eq_ignore_ascii_case(&b) || (a == '\\' && b == '/'))
}

/// `collectWorkspaceStats(folder, ['node_modules', '.git'])`: file types and config files of at most 20000 files.
pub fn collect_workspace_stats(folder: &Path) -> WorkspaceStats {
    let mut file_types = HashMap::new();
    let mut config_files = HashMap::new();
    let mut count = 0;
    let mut max_reached = false;
    collect(folder, folder, &mut file_types, &mut config_files, &mut count, &mut max_reached);
    WorkspaceStats {
        file_types: sorted(file_types),
        config_files: sorted(config_files),
        file_count: count,
        max_files_reached: max_reached,
        launch_config_files: collect_launch_configs(folder),
    }
}

/// `collect`: depth first in readdir order, skipping `node_modules` and `.git` folders and unreadable ones.
fn collect(root: &Path, dir: &Path, file_types: &mut HashMap<String, usize>, config_files: &mut HashMap<String, usize>, count: &mut usize, max_reached: &mut bool) {
    if *count >= MAX_FILES {
        *max_reached = true;
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let relative = dir.strip_prefix(root).unwrap_or(dir).to_string_lossy().into_owned();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // Dirent `isDirectory()`: a symlink is a file here.
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            if name != "node_modules" && name != ".git" {
                collect(root, &entry.path(), file_types, config_files, count, max_reached);
            }
            continue;
        }
        if *count >= MAX_FILES {
            *max_reached = true;
            return;
        }
        *count += 1;
        let file_type = name.rfind('.').map(|index| &name[index + 1..]).filter(|extension| !extension.is_empty()).unwrap_or(NO_EXT_KEY);
        *file_types.entry(file_type.to_owned()).or_default() += 1;
        for (tag, matches_name, matches_dir) in CONFIG_FILE_PATTERNS {
            if matches_dir.is_none_or(|matches_dir| matches_dir(&relative)) && matches_name(&name) {
                *config_files.entry((*tag).to_owned()).or_default() += 1;
            }
        }
    }
}

/// `asSortedItems`: highest count first; equal counts keep their first-seen order in upstream's Map, here name order.
fn sorted(items: HashMap<String, usize>) -> Vec<(String, usize)> {
    let mut items: Vec<_> = items.into_iter().collect();
    items.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    items
}

/// `collectLaunchConfigs`: the `type` of each configuration in `.vscode/launch.json`.
fn collect_launch_configs(folder: &Path) -> Vec<(String, usize)> {
    let Ok(text) = std::fs::read_to_string(folder.join(".vscode/launch.json")) else { return Vec::new() };
    let mut errors = Vec::new();
    let json = config::json::parse(&text, &mut errors, Default::default());
    if !errors.is_empty() {
        return Vec::new();
    }
    let mut types = HashMap::new();
    for configuration in json.as_ref().and_then(|json| json["configurations"].as_array()).into_iter().flatten() {
        if let Some(kind) = configuration["type"].as_str().filter(|kind| !kind.is_empty()) {
            *types.entry(kind.to_owned()).or_default() += 1;
        }
    }
    sorted(types)
}

/// The machine facts `formatEnvironment` reads from Node's `os`.
pub struct Machine {
    pub os_type: String,
    pub arch: String,
    pub release: String,
    pub cpu_model: String,
    pub cpu_count: usize,
    pub cpu_speed: u64,
    pub total_memory: f64,
    pub free_memory: f64,
    pub load_average: [f64; 3],
    pub vm_hint: f64,
}

/// `getDiagnostics`: the environment, the process tree, then the stats of each window's folders.
pub fn format_diagnostics(info: &MainDiagnostics, machine: &Machine, root: Option<&ProcessItem>, stats: &dyn Fn(&Path) -> WorkspaceStats) -> String {
    let mut output = vec![String::new(), format_environment(info, machine), String::new(), format_process_list(info, root)];
    if info.windows.iter().any(|window| !window.folders.is_empty()) {
        output.extend([String::new(), "Workspace Stats: ".into(), format_workspace_metadata(info, stats)]);
    }
    output.extend([String::new(), String::new()]);
    output.join("\n")
}

fn format_environment(info: &MainDiagnostics, machine: &Machine) -> String {
    let product = |key: &str| crate::product::get(key).as_str().map(str::to_owned);
    let gb = 1024. * 1024. * 1024.;
    let load: Vec<String> = machine.load_average.iter().map(|load| format!("{}", load.round())).collect();
    [
        format!("Version:          {} {} ({}, {})", product("nameShort").unwrap_or_default(), product("version").unwrap_or_default(), product("commit").unwrap_or("Commit unknown".into()), product("date").unwrap_or("Date unknown".into())),
        format!("OS Version:       {} {} {}", machine.os_type, machine.arch, machine.release),
        format!("CPUs:             {} ({} x {})", machine.cpu_model, machine.cpu_count, machine.cpu_speed),
        format!("Memory (System):  {:.2}GB ({:.2}GB free)", machine.total_memory / gb, machine.free_memory / gb),
        format!("Load (avg):       {}", load.join(", ")),
        format!("VM:               {}%", (machine.vm_hint * 100.).round()),
        format!("Screen Reader:    {}", if info.screen_reader { "yes" } else { "no" }),
        format!("Process Argv:     {}", info.main_arguments.join(" ")),
        // `expandGPUFeatures` of no features is the empty string.
        "GPU Status:       ".into(),
    ]
    .join("\n")
}

fn format_process_list(info: &MainDiagnostics, root: Option<&ProcessItem>) -> String {
    let mut names: HashMap<u32, String> = HashMap::new();
    for window in &info.windows {
        names.insert(window.pid, format!("window [{}] ({})", window.id, window.title));
    }
    for (pid, name) in &info.pid_to_names {
        names.insert(*pid, name.clone());
    }
    let mut output = vec!["CPU %\tMem MB\t   PID\tProcess".to_owned()];
    if let Some(root) = root {
        format_process_item(info.main_pid, &names, &mut output, root, 0);
    }
    output.join("\n")
}

fn format_process_item(main_pid: u32, names: &HashMap<u32, String>, output: &mut Vec<String>, item: &ProcessItem, indent: usize) {
    let name = match indent {
        0 if item.pid == main_pid => crate::product::get("applicationName").as_str().unwrap_or_default().to_owned(),
        0 => "remote-server".to_owned(),
        _ => names.get(&item.pid).cloned().unwrap_or_else(|| format!("{} {}", "  ".repeat(indent), item.name)),
    };
    output.push(format!("{:>5}\t{:>6}\t{:>6}\t{name}", format!("{:.0}", item.load.round()), format!("{:.0}", (item.mem / (1024. * 1024.)).round()), item.pid));
    for child in &item.children {
        format_process_item(main_pid, names, output, child, indent + 1);
    }
}

fn format_workspace_metadata(info: &MainDiagnostics, stats: &dyn Fn(&Path) -> WorkspaceStats) -> String {
    let mut output = Vec::new();
    for window in info.windows.iter().filter(|window| !window.folders.is_empty()) {
        output.push(format!("|  Window ({})", window.title));
        for folder in &window.folders {
            let stats = stats(folder);
            let count = match stats.max_files_reached {
                true => format!("more than {} files", stats.file_count),
                false => format!("{} files", stats.file_count),
            };
            let name = folder.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            output.push(format!("|    Folder ({name}): {count}"));
            output.push(format_workspace_stats(&stats));
        }
    }
    output.join("\n")
}

/// `formatWorkspaceStats`: the ten commonest file types and the rest as `other`, the config files, then the launch
/// configuration types, wrapped at 60 columns.
fn format_workspace_stats(stats: &WorkspaceStats) -> String {
    const LINE_LENGTH: usize = 60;
    let mut output = Vec::new();
    let mut line = "|      File types:".to_owned();
    let mut col = 0;
    let append_and_wrap = |line: &mut String, col: &mut usize, output: &mut Vec<String>, name: &str, count: usize| {
        let item = format!(" {name}({count})");
        if *col + item.len() > LINE_LENGTH {
            output.push(std::mem::replace(line, "|                 ".to_owned()));
            *col = line.len();
        } else {
            *col += item.len();
        }
        line.push_str(&item);
    };
    let named: Vec<_> = stats.file_types.iter().filter(|(name, _)| name != NO_EXT_KEY).collect();
    let mut other: usize = stats.file_types.iter().filter(|(name, _)| name == NO_EXT_KEY).map(|(_, count)| count).sum();
    for (index, (name, count)) in named.iter().enumerate() {
        match index < 10 {
            true => append_and_wrap(&mut line, &mut col, &mut output, name, *count),
            false => other += count,
        }
    }
    if other > 0 {
        append_and_wrap(&mut line, &mut col, &mut output, "other", other);
    }
    output.push(line);
    let mut line = "|      Conf files:".to_owned();
    col = 0;
    for (name, count) in &stats.config_files {
        append_and_wrap(&mut line, &mut col, &mut output, name, *count);
    }
    output.push(line);
    if !stats.launch_config_files.is_empty() {
        let mut line = "|      Launch Configs:".to_owned();
        for (name, count) in &stats.launch_config_files {
            line += &if *count > 1 { format!(" {name}({count})") } else { format!(" {name}") };
        }
        output.push(line);
    }
    output.join("\n")
}

/// The running machine, as Node's `os` reports it on macOS.
pub fn machine() -> Machine {
    let sysctl = |key: &str| std::process::Command::new("/usr/sbin/sysctl").args(["-n", key]).output().map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned()).unwrap_or_default();
    let mut load_average = [0f64; 3];
    // SAFETY: getloadavg writes at most the 3 elements it is given.
    unsafe { libc::getloadavg(load_average.as_mut_ptr(), 3) };
    Machine {
        os_type: "Darwin".into(),
        arch: std::env::consts::ARCH.replace("aarch64", "arm64").replace("x86_64", "x64"),
        release: sysctl("kern.osrelease"),
        cpu_model: sysctl("machdep.cpu.brand_string"),
        cpu_count: sysctl("hw.ncpu").parse().unwrap_or_default(),
        // libuv reads hw.cpufrequency, which Apple silicon does not have; 0 is what Node reports there too.
        cpu_speed: sysctl("hw.cpufrequency").parse::<u64>().map(|hz| hz / 1_000_000).unwrap_or_default(),
        total_memory: sysctl("hw.memsize").parse().unwrap_or_default(),
        free_memory: crate::workbench::perf_view::free_memory(),
        load_average,
        vm_hint: virtual_machine_hint(),
    }
}

/// base/node/id.ts `virtualMachineHint`: the share of external interface addresses whose MAC has a VM vendor prefix.
fn virtual_machine_hint() -> f64 {
    const VM_OUIS: [&str; 7] = ["00:50:56", "00:0C:29", "00:05:69", "00:03:FF", "00:1C:42", "00:16:3E", "08:00:27"];
    let interfaces = network_interfaces();
    let external: Vec<&(String, bool)> = interfaces.iter().filter(|(_, internal)| !internal).collect();
    if external.is_empty() {
        return 0.;
    }
    let virtual_count = external.iter().filter(|(mac, _)| VM_OUIS.iter().any(|oui| mac.to_uppercase().contains(oui))).count();
    virtual_count as f64 / external.len() as f64
}

/// Node's `os.networkInterfaces()` flattened: one entry per IPv4 or IPv6 address with its interface's MAC and
/// whether it is a loopback (`internal`).
fn network_interfaces() -> Vec<(String, bool)> {
    let mut macs: HashMap<String, String> = HashMap::new();
    let mut addresses: Vec<(String, bool)> = Vec::new();
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills a list freed below with freeifaddrs; each node is read while the list lives.
    unsafe {
        if libc::getifaddrs(&mut list) != 0 {
            return Vec::new();
        }
        let mut node = list;
        while !node.is_null() {
            let entry = &*node;
            node = entry.ifa_next;
            if entry.ifa_addr.is_null() {
                continue;
            }
            let name = std::ffi::CStr::from_ptr(entry.ifa_name).to_string_lossy().into_owned();
            match (*entry.ifa_addr).sa_family as i32 {
                libc::AF_LINK => {
                    let link = &*(entry.ifa_addr as *const libc::sockaddr_dl);
                    let start = link.sdl_nlen as usize;
                    // `sdl_data` is declared 12 bytes but runs past them: the name, then the address.
                    let data = link.sdl_data.as_ptr() as *const u8;
                    let bytes: Vec<u8> = (0..link.sdl_alen as usize).map(|index| *data.add(start + index)).collect();
                    if bytes.len() == 6 {
                        macs.insert(name, bytes.iter().map(|byte| format!("{byte:02x}")).collect::<Vec<_>>().join(":"));
                    }
                }
                libc::AF_INET | libc::AF_INET6 => addresses.push((name, entry.ifa_flags & libc::IFF_LOOPBACK as u32 != 0)),
                _ => {}
            }
        }
        libc::freeifaddrs(list);
    }
    addresses.into_iter().map(|(name, internal)| (macs.get(&name).cloned().unwrap_or_else(|| "00:00:00:00:00:00".into()), internal)).collect()
}

/// The second instance's `--status`: the running instance's facts printed as a report; without one, the warning.
pub fn print_status(socket: &Path) {
    let name_short = crate::product::get("nameShort").as_str().unwrap_or_default();
    let Some(info) = crate::launch::request_main_diagnostics(socket) else {
        println!("Warning: The --status argument can only be used if {name_short} is already running. Please run it again after {name_short} has started.");
        return;
    };
    let machine = machine();
    let root = crate::ps::list_processes(info.main_pid, machine.total_memory).ok();
    println!("{}", format_diagnostics(&info, &machine, root.as_ref(), &|folder| collect_workspace_stats(folder)));
}

#[cfg(test)]
mod tests;
