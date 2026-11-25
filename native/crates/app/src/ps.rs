//! base/node/ps.ts `listProcesses` on macOS: `ps -ax -o pid=,ppid=,pcpu=,pmem=,command=` turned into the process
//! tree under a root pid, each process named as `findName` names it. The process explorer (processExplorerControl.ts)
//! draws this tree.

use std::process::Command;

/// `ProcessItem`: CPU load in percent, memory in bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcessItem {
    pub name: String,
    pub cmd: String,
    pub pid: u32,
    pub ppid: u32,
    pub load: f64,
    pub mem: f64,
    pub children: Vec<ProcessItem>,
}

/// Runs `ps` (with the `LC_NUMERIC` ps.ts sets, so `.` separates decimals) and returns the tree under `root_pid`.
pub fn list_processes(root_pid: u32, total_memory: f64) -> Result<ProcessItem, String> {
    let output = Command::new("/bin/ps").args(["-ax", "-o", "pid=,ppid=,pcpu=,pmem=,command="]).env("LC_NUMERIC", "en_US.UTF-8").output().map_err(|error| error.to_string())?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    // ps.ts ignores this one (microsoft/vscode#98590).
    if !output.status.success() || !stderr.is_empty() && !stderr.contains("screen size is bogus") {
        return Err(stderr.into_owned());
    }
    process_tree(&String::from_utf8_lossy(&output.stdout), root_pid, total_memory).ok_or_else(|| format!("Root process {root_pid} not found"))
}

/// `parsePsOutput` with `addToTree`: a process joins the tree when it is the root or its parent already joined, as
/// ps lists parents before children; children are sorted by pid.
pub fn process_tree(stdout: &str, root_pid: u32, total_memory: f64) -> Option<ProcessItem> {
    let mut items: Vec<ProcessItem> = Vec::new();
    for line in stdout.lines() {
        let mut fields = line.split_whitespace();
        let (Some(pid), Some(ppid), Some(load), Some(mem)) = (fields.next(), fields.next(), fields.next(), fields.next()) else { continue };
        let (Ok(pid), Ok(ppid), Ok(load), Ok(mem)) = (pid.parse::<u32>(), ppid.parse::<u32>(), load.parse::<f64>(), mem.parse::<f64>()) else { continue };
        let cmd = fields.collect::<Vec<_>>().join(" ");
        if cmd.is_empty() || pid != root_pid && !items.iter().any(|item| item.pid == ppid) {
            continue;
        }
        items.push(ProcessItem { name: find_name(&cmd), cmd, pid, ppid, load, mem: total_memory * (mem / 100.), children: Vec::new() });
    }
    let root = items.iter().position(|item| item.pid == root_pid)?;
    let root = items.swap_remove(root);
    Some(attach_children(root, &mut items))
}

fn attach_children(mut parent: ProcessItem, rest: &mut Vec<ProcessItem>) -> ProcessItem {
    let mut children = Vec::new();
    let mut index = 0;
    while index < rest.len() {
        if rest[index].ppid == parent.pid {
            children.push(rest.remove(index));
        } else {
            index += 1;
        }
    }
    children.sort_by_key(|child| child.pid);
    parent.children = children.into_iter().map(|child| attach_children(child, rest)).collect();
    parent
}

/// ps.ts `findName`: the Electron `--type=` of a process, the `.js` files of a non-node command, else the command.
pub fn find_name(cmd: &str) -> String {
    if cmd.contains("--crashes-directory") {
        return "electron-crash-reporter".into();
    }
    if let Some(kind) = cmd.split_whitespace().find_map(|arg| arg.strip_prefix("--type=")).map(|kind| kind.split(|char: char| !(char.is_ascii_alphabetic() || char == '-')).next().unwrap_or_default()).filter(|kind| !kind.is_empty()) {
        return match kind {
            "renderer" => "window".into(),
            "utility" if cmd.to_lowercase().contains("--utility-sub-type=network") => "utility-network-service".into(),
            "utility" => "utility-process".into(),
            "extensionHost" => "extension-host".into(),
            kind => kind.into(),
        };
    }
    if !cmd.contains("node ") && !cmd.contains("node.exe") {
        // JS_FILENAME_PATTERN: /[a-zA-Z-]+\.js\b/g
        let scripts: Vec<&str> = cmd.split(|char: char| !(char.is_ascii_alphabetic() || char == '-' || char == '.')).filter(|word| word.len() > 3 && word.ends_with(".js") && word[..word.len() - 3].chars().all(|char| char.is_ascii_alphabetic() || char == '-')).collect();
        if !scripts.is_empty() {
            return format!("electron-nodejs ({})", scripts.join(" "));
        }
    }
    cmd.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tree_keeps_the_descendants_of_the_root_sorted_by_pid() {
        let stdout = "  1     0   0.0  0.1 /sbin/launchd\n 100     1  12.5  1.0 /Applications/VSRustium.app/Contents/MacOS/vsrustium\n 300   100   3.0  0.5 bun out-exthost/bootstrap-exthost.js --type=extensionHost\n 200   100   0.0  0.2 /bin/zsh -l\n 400   300   0.0  0.1 node /x/server.js\n 500     1   0.0  0.1 /usr/bin/other\n";
        let root = process_tree(stdout, 100, 1000.).unwrap();
        assert_eq!((root.pid, root.load, root.mem), (100, 12.5, 10.));
        let children: Vec<(u32, &str)> = root.children.iter().map(|child| (child.pid, child.name.as_str())).collect();
        assert_eq!(children, [(200, "/bin/zsh -l"), (300, "extension-host")]);
        assert_eq!(root.children[1].children[0].name, "node /x/server.js");
        assert!(process_tree(stdout, 9, 1000.).is_none());
    }

    #[test]
    fn ps_finds_this_process() {
        let root = list_processes(std::process::id(), 1000.).unwrap();
        assert!(root.cmd.contains("vsrustium"), "{}", root.cmd);
    }

    #[test]
    fn names_follow_find_name() {
        assert_eq!(find_name("Code Helper --type=renderer --foo"), "window");
        assert_eq!(find_name("Code Helper --type=utility --utility-sub-type=network.mojom.NetworkService"), "utility-network-service");
        assert_eq!(find_name("Code Helper --type=gpu-process"), "gpu-process");
        assert_eq!(find_name("electron out/cli.js --x watcher.js"), "electron-nodejs (cli.js watcher.js)");
        assert_eq!(find_name("node server.js"), "node server.js");
    }
}
