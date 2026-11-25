//! outputChannelModel.ts `MultiFileContentProvider` and outputServices.ts `registerCompoundLogChannel`: a log
//! channel that reads several log files, each entry tagged with its source's name, new entries ordered by time.

use super::filter::{LOG_ENTRY, parse_log_entries};
use super::{Content, Output, OutputChannel};

/// One source of a compound channel: its file, read as a channel of its own, and the name its entries get.
pub struct Source {
    pub reader: OutputChannel,
    pub name: String,
}

impl Output {
    /// `registerCompoundLogChannel`: the channels sorted by label, the id their ids lowercased and joined by `-`,
    /// registered once as a user channel. Returns the id.
    pub fn register_compound_channel(&mut self, ids: &[String]) -> String {
        let mut channels: Vec<&OutputChannel> = ids.iter().filter_map(|id| self.channel(id)).collect();
        channels.sort_by(|a, b| crate::command_palette::locale_compare(&a.label, &b.label));
        let id = channels.iter().map(|channel| channel.id.to_lowercase()).collect::<Vec<_>>().join("-");
        if self.channel(&id).is_some() {
            return id;
        }
        let label = channels.iter().map(|channel| channel.label.as_str()).collect::<Vec<_>>().join(", ");
        let log = channels.iter().any(|channel| channel.log);
        let sources = channels.iter().flat_map(|channel| channel_sources(channel)).collect();
        let channel = OutputChannel { user: true, sources, ..OutputChannel::new(id.clone(), label, Default::default(), log) };
        self.register_channel(channel);
        id
    }
}

/// A single-file channel is one source named by its label; a compound one contributes its own sources.
fn channel_sources(channel: &OutputChannel) -> Vec<Source> {
    if !channel.sources.is_empty() {
        return channel.sources.iter().map(|source| Source { reader: OutputChannel::new(String::new(), String::new(), source.reader.file.clone(), true), name: source.name.clone() }).collect();
    }
    vec![Source { reader: OutputChannel::new(String::new(), String::new(), channel.file.clone(), true), name: channel.label.clone() }]
}

impl OutputChannel {
    /// `MultiFileContentProvider.getContent`: what every source gained, as log entries with ` [name]` after their
    /// level, the batch ordered by time and appended. A shrunk file reads every source again.
    pub(super) fn read_sources(&mut self) -> Content {
        let mut reset = false;
        let mut texts = Vec::new();
        for source in &mut self.sources {
            match source.reader.read_appended() {
                Content::Append(text) => texts.push(text),
                Content::Reset(_) => reset = true,
            }
        }
        if reset {
            for source in &mut self.sources {
                source.reader.reset(Some(0));
            }
            return Content::Reset(self.read_sources_appended());
        }
        Content::Append(combine(texts.iter().zip(&self.sources).map(|(text, source)| (text.as_str(), source.name.as_str()))))
    }

    fn read_sources_appended(&mut self) -> String {
        match self.read_sources() {
            Content::Append(text) | Content::Reset(text) => text,
        }
    }
}

/// `combineLogEntries`: each source's entries tagged with its name, then all of them ordered by timestamp, each
/// ending in a line break. Text before a source's first entry is left out, as `logEntryIterator` skips it.
pub fn combine<'a>(outputs: impl Iterator<Item = (&'a str, &'a str)>) -> String {
    let mut entries: Vec<(i64, String)> = Vec::new();
    for (text, name) in outputs {
        let lines: Vec<String> = text.split('\n').skip_while(|line| !LOG_ENTRY.is_match(line)).map(str::to_owned).collect();
        for entry in parse_log_entries(&lines) {
            let header = &lines[entry.lines.start];
            let level_end = LOG_ENTRY.captures(header).and_then(|captures| captures.get(2)).map_or(0, |level| level.end());
            let mut content = format!("{} [{name}]{}", &header[..level_end], &header[level_end..]);
            for line in &lines[entry.lines.start + 1..entry.lines.end] {
                content.push('\n');
                content.push_str(line);
            }
            entries.push((entry.timestamp, content));
        }
    }
    entries.sort_by_key(|(timestamp, _)| *timestamp);
    let mut content = String::new();
    for (_, entry) in entries {
        content.push_str(&entry);
        content.push('\n');
    }
    content
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_of_two_logs_are_tagged_and_ordered_by_time() {
        let window = "2025-01-02 03:04:05.100 [info] window started\n2025-01-02 03:04:05.300 [error] window failed\n  at line\n";
        let git = "header text\n2025-01-02 03:04:05.200 [info] [git] git started\n";
        assert_eq!(
            combine([(window, "Window"), (git, "Git")].into_iter()),
            "2025-01-02 03:04:05.100 [info] [Window] window started\n2025-01-02 03:04:05.200 [info] [Git] [git] git started\n2025-01-02 03:04:05.300 [error] [Window] window failed\n  at line\n"
        );
    }

    #[test]
    fn a_compound_channel_is_registered_once_by_its_sorted_ids() {
        let mut output = Output::default();
        output.register_channel(OutputChannel::new("rendererLog".into(), "Window".into(), "/logs/renderer.log".into(), true));
        output.register_channel(OutputChannel::new("exthost".into(), "Extension Host".into(), "/logs/exthost.log".into(), true));
        let id = output.register_compound_channel(&["rendererLog".into(), "exthost".into()]);
        assert_eq!(id, "exthost-rendererlog");
        let channel = output.channel(&id).unwrap();
        assert_eq!((channel.label.as_str(), channel.user, channel.log), ("Extension Host, Window", true, true));
        assert_eq!(channel.sources.iter().map(|source| source.name.as_str()).collect::<Vec<_>>(), ["Extension Host", "Window"]);
        assert_eq!(output.register_compound_channel(&["exthost".into(), "rendererLog".into()]), id);
        assert_eq!(output.channels.len(), 3);
    }
}
