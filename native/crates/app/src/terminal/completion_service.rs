//! terminalCompletionService.ts without the extension host round trip: the provider registry and its selection,
//! file and folder completions read with `std::fs`, and the merge of a provider's items with them.

use std::fs;

use serde_json::Value;

use super::completion_item::{TerminalCompletion, TerminalCompletionItemKind};

#[cfg(test)]
mod tests;

/// `ITerminalCompletionProvider` as registered; the caller asks the extension host for its completions.
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalCompletionProvider {
    pub id: String,
    pub trigger_characters: Vec<String>,
    pub shell_types: Option<Vec<String>>,
    pub is_builtin: bool,
}

/// `TerminalCompletionService._providers`: providers by extension id, in registration order.
#[derive(Default)]
pub struct CompletionProviders {
    by_extension: Vec<(String, Vec<TerminalCompletionProvider>)>,
}

impl CompletionProviders {
    /// `registerTerminalCompletionProvider`: a provider with a registered id replaces it in place.
    pub fn register(&mut self, extension_id: &str, provider: TerminalCompletionProvider) {
        let Some((_, providers)) = self.by_extension.iter_mut().find(|(id, _)| id == extension_id) else {
            self.by_extension.push((extension_id.to_owned(), vec![provider]));
            return;
        };
        match providers.iter_mut().find(|existing| existing.id == provider.id) {
            Some(existing) => *existing = provider,
            None => providers.push(provider),
        }
    }

    /// The disposable `registerTerminalCompletionProvider` returns.
    pub fn unregister(&mut self, extension_id: &str, id: &str) {
        let Some(index) = self.by_extension.iter().position(|(extension, _)| extension == extension_id) else { return };
        let providers = &mut self.by_extension[index].1;
        providers.retain(|provider| provider.id != id);
        if providers.is_empty() {
            self.by_extension.remove(index);
        }
    }

    /// mainThreadTerminalService.ts `$unregisterCompletionProvider`: the provider by its id alone.
    pub fn unregister_provider(&mut self, id: &str) {
        let extension = self.by_extension.iter().find(|(_, providers)| providers.iter().any(|provider| provider.id == id)).map(|(extension, _)| extension.clone());
        if let Some(extension) = extension {
            self.unregister(&extension, id);
        }
    }

    pub fn providers(&self) -> impl Iterator<Item = &TerminalCompletionProvider> {
        self.by_extension.iter().flat_map(|(_, providers)| providers)
    }

    /// `provideCompletions` and `_collectCompletions` up to the requests: the providers to ask for completions.
    /// `provider_config` is the `terminal.integrated.suggest.providers` object.
    pub fn providers_to_request(&self, prompt_value: &str, cursor_position: usize, trigger_character: bool, skip_extension_completions: bool, shell_type: Option<&str>, provider_config: &Value) -> Vec<&TerminalCompletionProvider> {
        let prefix: String = prompt_value.chars().take(cursor_position).collect();
        let providers: Vec<_> = match trigger_character {
            true => self.providers().filter(|provider| provider.trigger_characters.iter().any(|character| prefix.ends_with(character.as_str()))).collect(),
            false => self.providers().collect(),
        };
        let providers = match skip_extension_completions {
            true => providers.into_iter().filter(|provider| provider.is_builtin).collect(),
            false => get_enabled_providers(providers, provider_config),
        };
        providers
            .into_iter()
            .filter(|provider| match (&provider.shell_types, shell_type) {
                (Some(shell_types), Some(shell_type)) => shell_types.iter().any(|supported| supported == shell_type),
                _ => true,
            })
            .collect()
    }
}

/// `_getEnabledProviders`: providers not set to `false` in the `terminal.integrated.suggest.providers` object.
pub fn get_enabled_providers<'a>(providers: Vec<&'a TerminalCompletionProvider>, provider_config: &Value) -> Vec<&'a TerminalCompletionProvider> {
    providers.into_iter().filter(|provider| !provider.id.is_empty() && provider_config.get(&provider.id) != Some(&Value::Bool(false))).collect()
}

/// `TerminalCompletionResourceOptions`. Paths are POSIX: the native port reads the local file system only.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TerminalCompletionResourceOptions {
    pub show_files: bool,
    pub show_directories: bool,
    /// A glob string or an `IRelativePattern` object.
    pub glob_pattern: Option<Value>,
    pub cwd: String,
}

impl TerminalCompletionResourceOptions {
    /// A `TerminalCompletionResourceOptionsDto`. `None` for a non-`file` cwd or a `\` path separator.
    /// shortcut: remote and Windows file systems are not read, add them with a remote file service.
    pub fn from_dto(dto: &Value) -> Option<Self> {
        if dto["cwd"]["scheme"] != "file" || dto["pathSeparator"] != "/" {
            return None;
        }
        Some(Self {
            show_files: dto["showFiles"].as_bool() == Some(true),
            show_directories: dto["showDirectories"].as_bool() == Some(true),
            glob_pattern: Some(dto["globPattern"].clone()).filter(|pattern| !pattern.is_null()),
            cwd: dto["cwd"]["path"].as_str()?.to_owned(),
        })
    }
}

/// `TerminalCompletionList`: one provider's answer.
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalCompletionList {
    pub items: Vec<TerminalCompletion>,
    pub resource_options: Option<TerminalCompletionResourceOptions>,
}

impl TerminalCompletionList {
    /// A `TerminalCompletionListDto` as mainThreadTerminalService.ts maps it, items from provider `ext:<id>`.
    pub fn from_dto(dto: &Value, provider_id: &str, prompt_value: &str) -> Self {
        let provider = format!("ext:{provider_id}");
        let items = dto["items"].as_array().into_iter().flatten().filter_map(|item| TerminalCompletion::from_dto(item, &provider, prompt_value)).collect();
        Self { items, resource_options: TerminalCompletionResourceOptions::from_dto(&dto["resourceOptions"]) }
    }
}

/// `_collectCompletions` after the requests: each provider's items with its file and folder completions,
/// skipping those whose label an item already has.
pub fn collect(results: Vec<(&TerminalCompletionProvider, TerminalCompletionList)>, prompt_value: &str, cursor_position: usize, shell_type: Option<&str>, env: &dyn Fn(&str) -> Option<String>, cdpath_setting: &str) -> Vec<TerminalCompletion> {
    let mut completions = Vec::new();
    for (provider, list) in results {
        let mut items = list.items;
        for item in &mut items {
            if shell_type == Some("pwsh") {
                let start = item.replacement_range.map_or(0, |(start, _)| start);
                item.is_file_override.get_or_insert(item.kind == Some(TerminalCompletionItemKind::Method) && start == 0);
            }
            if provider.is_builtin && item.provider.is_empty() {
                item.provider = provider.id.clone();
            }
        }
        if let Some(options) = &list.resource_options {
            let resource_provider = format!("core:path:ext:{}", provider.id);
            if let Some(resources) = resolve_resources(options, prompt_value, cursor_position, &resource_provider, shell_type, env, cdpath_setting) {
                // Object labels never equal a resource's string label, as upstream compares them by identity.
                let labels: Vec<String> = items.iter().filter(|item| item.string_label).map(|item| item.label.clone()).collect();
                items.extend(resources.into_iter().filter(|resource| !labels.contains(&resource.label)));
            }
        }
        completions.extend(items);
    }
    completions
}

#[derive(Clone, Copy, PartialEq)]
enum PathType {
    Tilde,
    Absolute,
    Relative,
}

/// `resolveResources`: the folder the last word points into, its children, `$CDPATH` folders after `cd `, the
/// parent folder and `~`. `cursor_position` and the replacement ranges are char indices into `prompt_value`;
/// `env` reads the shell's environment, `cdpath_setting` is `terminal.integrated.suggest.cdPath`.
pub fn resolve_resources(
    options: &TerminalCompletionResourceOptions,
    prompt_value: &str,
    cursor_position: usize,
    provider: &str,
    shell_type: Option<&str>,
    env: &dyn Fn(&str) -> Option<String>,
    cdpath_setting: &str,
) -> Option<Vec<TerminalCompletion>> {
    // Files imply folders, the file could be in any folder.
    if !options.show_directories && !options.show_files {
        return None;
    }

    let cursor_prefix: String = prompt_value.chars().take(cursor_position).collect();
    let words = split_unescaped_spaces(&cursor_prefix);
    let is_command_position = words.len() <= 1 && !cursor_prefix.ends_with(' ');
    let mut last_word = if cursor_prefix.ends_with(' ') { "" } else { words[words.len() - 1] };
    // `FOO=./src` completes `./src`.
    if let Some((name, value)) = last_word.split_once('=')
        && !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphabetic() || c == '_')
        && !value.is_empty()
        && !value.contains('\n')
    {
        last_word = value;
    }
    let replacement_range = Some((cursor_position - last_word.chars().count(), cursor_position));
    let completion = |label: String, kind: TerminalCompletionItemKind, detail: String| TerminalCompletion {
        kind: Some(kind),
        detail: Some(detail),
        replacement_range,
        ..TerminalCompletion::new(label, provider)
    };

    // The folder part of the last word, `./src/` of `./src/file`.
    let last_word_folder = last_word.rfind('/').map_or("", |index| &last_word[..=index]);
    let has_dot_prefix = ["./", "../", ".\\", "..\\"].iter().any(|prefix| last_word_folder.starts_with(prefix));
    let path_type = match last_word_folder {
        folder if folder.starts_with('~') => PathType::Tilde,
        folder if folder.starts_with('/') => PathType::Absolute,
        _ => PathType::Relative,
    };
    let cwd = options.cwd.as_str();
    let folder = match path_type {
        PathType::Relative if last_word_folder.is_empty() => cwd.to_owned(),
        PathType::Relative => resolve_relative_folder(cwd, &last_word_folder.replace("\\ ", " "))?,
        PathType::Tilde => match env("HOME") {
            Some(home) => join_path(&home, &last_word_folder[1..].replace("\\ ", " ")),
            None if last_word == "~/" => return Some(vec![completion(last_word_folder.to_owned(), TerminalCompletionItemKind::Folder, "$HOME".to_owned())]),
            None => return None,
        },
        PathType::Absolute => last_word_folder.replace("\\ ", " "),
    };
    let children = read_children(&folder)?;

    // The folder itself comes first: it is an exact match. Relative `|` completes `.` without a trailing `/`
    // so `runOnEnter` does not run `./`.
    let mut completions = Vec::new();
    let folder_label = match path_type {
        PathType::Relative if last_word_folder.is_empty() => ".".to_owned(),
        PathType::Relative => add_path_relative_prefix(last_word_folder, has_dot_prefix),
        _ => last_word_folder.to_owned(),
    };
    completions.push(completion(folder_label, TerminalCompletionItemKind::Folder, friendly_path(&folder, TerminalCompletionItemKind::Folder)));

    for child in children {
        let kind = match (child.is_directory, child.is_symbolic_link) {
            (true, true) => TerminalCompletionItemKind::SymbolicLinkFolder,
            (true, false) => TerminalCompletionItemKind::Folder,
            // Only executables complete the command on Unix.
            _ if !options.show_files || (is_command_position && !child.executable) => continue,
            (false, true) => TerminalCompletionItemKind::SymbolicLinkFile,
            (false, false) => TerminalCompletionItemKind::File,
        };
        let mut label = last_word_folder.to_owned();
        if !label.is_empty() && !label.ends_with('/') {
            label.push('/');
        }
        label.push_str(&child.name);
        if path_type == PathType::Relative {
            label = add_path_relative_prefix(&label, has_dot_prefix);
        }
        if child.is_directory && !label.ends_with('/') {
            label.push('/');
        }
        let label = escape_terminal_completion_label(&label, shell_type);
        if !child.is_directory && options.glob_pattern.as_ref().is_some_and(|pattern| !matches_glob(pattern, &child.path)) {
            continue;
        }
        let mut detail = friendly_path(&child.path, kind);
        // A dangling link has no target to show.
        if child.is_symbolic_link
            && let Ok(target) = fs::canonicalize(&child.path)
            && target.to_string_lossy() != child.path
        {
            detail = format!("{detail} -> {}", friendly_path(&target.to_string_lossy(), kind));
        }
        completions.push(completion(label, kind, detail));
    }

    // `$CDPATH` folders, only for `cd`.
    let cdpath = env("CDPATH").filter(|_| path_type == PathType::Relative && prompt_value.starts_with("cd ") && matches!(cdpath_setting, "absolute" | "relative"));
    if let Some(cdpath) = cdpath {
        for entry in cdpath.split(':').filter(|entry| !entry.is_empty()) {
            let Some(children) = read_children(&normalize_posix(&format!("/{entry}"))) else { continue };
            for child in children.into_iter().filter(|child| child.is_directory) {
                let friendly = friendly_path(&child.path, TerminalCompletionItemKind::Folder);
                let (label, detail) = match cdpath_setting {
                    "relative" => (child.name, format!("CDPATH {friendly}")),
                    _ => (friendly, "CDPATH".to_owned()),
                };
                completions.push(completion(label, TerminalCompletionItemKind::Folder, detail));
            }
        }
    }

    // The parent folder goes last, it is rarely the one wanted.
    if path_type == PathType::Relative {
        let label = match last_word_folder {
            "" => "../".to_owned(),
            folder => add_path_relative_prefix(&format!("{folder}../"), has_dot_prefix),
        };
        completions.push(completion(label, TerminalCompletionItemKind::Folder, friendly_path(&join_path(&folder, "../"), TerminalCompletionItemKind::Folder)));
    }

    // `~` when the word has no folder yet.
    if path_type == PathType::Relative && !last_word_folder.contains(['/', '\\']) {
        let detail = match env("HOME") {
            Some(home) => friendly_path(&normalize_posix(&home), TerminalCompletionItemKind::Folder),
            None => "$HOME".to_owned(),
        };
        completions.push(completion("~".to_owned(), TerminalCompletionItemKind::Folder, detail));
    }

    Some(completions)
}

/// `escapeTerminalCompletionLabel`: backslash-escapes glob and shell metacharacters, except for pwsh and cmd.
pub fn escape_terminal_completion_label(label: &str, shell_type: Option<&str>) -> String {
    if matches!(shell_type, None | Some("pwsh") | Some("cmd")) {
        return label.to_owned();
    }
    let mut escaped = String::with_capacity(label.len());
    for c in label.chars() {
        if "[]()'\"\\`*?;|&<>".contains(c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

/// The folder a relative last word points into. A `./src/vs/` that names the tail of the cwd is the cwd itself,
/// as extensions pass the cwd already resolved.
fn resolve_relative_folder(cwd: &str, folder: &str) -> Option<String> {
    if let Some(rest) = folder.strip_prefix("./") {
        let stripped = rest.trim_start_matches('/').trim_end_matches('/');
        let cwd_parts: Vec<&str> = cwd.trim_end_matches('/').split('/').collect();
        let stripped_parts: Vec<&str> = stripped.split('/').collect();
        let tail_matches = stripped_parts.len() <= cwd_parts.len() && cwd_parts[cwd_parts.len() - stripped_parts.len()..] == stripped_parts[..];
        if stripped.is_empty() || tail_matches {
            return fs::metadata(cwd).is_ok().then(|| cwd.to_owned());
        }
    }
    let resolved = join_path(cwd, folder);
    fs::metadata(&resolved).is_ok().then_some(resolved)
}

struct Child {
    name: String,
    path: String,
    is_directory: bool,
    is_symbolic_link: bool,
    executable: bool,
}

/// The children of `folder`, by name. A dangling link is neither a file nor a folder and is left out.
/// shortcut: sorted by name for a stable order, upstream keeps the readdir order; the suggest model sorts anyway.
fn read_children(folder: &str) -> Option<Vec<Child>> {
    use std::os::unix::fs::PermissionsExt;
    let mut children = Vec::new();
    for entry in fs::read_dir(folder).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = join_path(folder, &name);
        let Ok(metadata) = fs::metadata(&path) else { continue };
        if !metadata.is_dir() && !metadata.is_file() {
            continue;
        }
        let is_symbolic_link = fs::symlink_metadata(&path).is_ok_and(|link| link.file_type().is_symlink());
        children.push(Child { name, path, is_directory: metadata.is_dir(), is_symbolic_link, executable: metadata.permissions().mode() & 0o111 != 0 });
    }
    children.sort_by(|a, b| a.name.cmp(&b.name));
    Some(children)
}

/// `getFriendlyPath`: `getUriLabel(uri, { noPrefix: true })`, which on macOS and Linux is the normalized path
/// with the user home tildified, and a trailing `/` for folders.
fn friendly_path(path: &str, kind: TerminalCompletionItemKind) -> String {
    let mut label = normalize_posix(&tildify(path));
    if kind == TerminalCompletionItemKind::Folder && !label.ends_with('/') {
        label.push('/');
    }
    label
}

/// labels.ts `tildify` with the user home of this process; case-insensitive except on Linux.
fn tildify(path: &str) -> String {
    let Ok(home) = std::env::var("HOME") else { return path.to_owned() };
    let home = format!("{}/", home.trim_end_matches('/'));
    let Some(head) = path.get(..home.len()) else { return path.to_owned() };
    let in_home = if cfg!(target_os = "linux") { head == home } else { head.eq_ignore_ascii_case(&home) };
    match in_home {
        true => format!("~/{}", &path[home.len()..]),
        false => path.to_owned(),
    }
}

/// `addPathRelativePrefix`: `./` in front of a relative path the user typed without one.
fn add_path_relative_prefix(text: &str, has_dot_prefix: bool) -> String {
    match (has_dot_prefix, text.starts_with('/')) {
        (true, _) => text.to_owned(),
        (false, true) => format!(".{text}"),
        (false, false) => format!("./{text}"),
    }
}

/// `text.split(/(?<!\\) /)`.
fn split_unescaped_spaces(text: &str) -> Vec<&str> {
    let mut words = Vec::new();
    let mut start = 0;
    let mut previous = None;
    for (index, c) in text.char_indices() {
        if c == ' ' && previous != Some('\\') {
            words.push(&text[start..index]);
            start = index + 1;
        }
        previous = Some(c);
    }
    words.push(&text[start..]);
    words
}

/// `URI.joinPath` of a POSIX path.
fn join_path(base: &str, relative: &str) -> String {
    normalize_posix(&format!("{base}/{relative}"))
}

/// `posix.normalize` of an absolute path: `.` and `..` resolved, repeated `/` collapsed, a trailing `/` kept.
fn normalize_posix(path: &str) -> String {
    let mut segments = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            _ => segments.push(segment),
        }
    }
    let mut normalized = format!("/{}", segments.join("/"));
    if path.ends_with('/') && !normalized.ends_with('/') {
        normalized.push('/');
    }
    normalized
}

/// glob.ts `match` of a pattern string or an `IRelativePattern` against a file path, ignoring case where the
/// file system does (`PathCaseSensitive` is set on Linux only).
fn matches_glob(pattern: &Value, path: &str) -> bool {
    let options = files::glob::Options { ignore_case: !cfg!(target_os = "linux"), ..Default::default() };
    if let Some(pattern) = pattern.as_str() {
        return files::glob::match_pattern(pattern, path, options);
    }
    let base = pattern["baseUri"]["path"].as_str().or(pattern["base"].as_str());
    let (Some(base), Some(relative_pattern)) = (base, pattern["pattern"].as_str()) else { return false };
    let base = base.trim_end_matches('/');
    let Some(head) = path.get(..base.len()) else { return false };
    let in_base = if options.ignore_case { head.eq_ignore_ascii_case(base) } else { head == base };
    match path[base.len()..].strip_prefix('/') {
        Some(relative) if in_base => files::glob::match_pattern(relative_pattern, relative, options),
        _ => false,
    }
}
