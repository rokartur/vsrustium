//! codelensController.ts and codelensWidget.ts: the active editor's code lenses from every `$registerCodeLensSupport`
//! provider, grouped per line (codelens.ts `getCodeLensModel` order) into a view zone above the line, each command a
//! link that runs it; unresolved lenses get `$resolveCodeLens`.

use gpui::{Context, Div, MouseButton, MouseDownEvent, div, prelude::*, px};
use serde_json::{Value, json};

use super::Workbench;
use crate::editor::code_lenses::Lens;
use crate::editor::{OpenEditor, Position};
use crate::language_features::Candidate;

/// codelensController.ts `_getLayoutInfo`: `editor.codeLensFontSize` or 90% of the font size, and its height.
pub fn layout(font_size: f32, line_height: f32, code_lens_font_size: f32) -> (f32, f32) {
    let factor = (line_height / font_size).max(1.3);
    let size = if code_lens_font_size >= 5. { code_lens_font_size } else { (font_size * 0.9).trunc() };
    (size, (size * factor).trunc())
}

/// `getCodeLensModel` sort: by line, provider rank, column; then grouped by start line.
pub fn group_lenses(mut lenses: Vec<(usize, Lens)>) -> Vec<(usize, Vec<Lens>)> {
    let start = |lens: &Lens, key: &str| lens.symbol["range"][key].as_u64().unwrap_or_default();
    lenses.sort_by_key(|(rank, lens)| (start(lens, "startLineNumber"), *rank, start(lens, "startColumn")));
    let mut groups: Vec<(usize, Vec<Lens>)> = Vec::new();
    for (_, lens) in lenses {
        let line = (start(&lens, "startLineNumber") as usize).saturating_sub(1);
        match groups.last_mut() {
            Some((last, group)) if *last == line => group.push(lens),
            _ => groups.push((line, vec![lens])),
        }
    }
    groups
}

impl Workbench {
    fn code_lens_layout(&self) -> (f32, f32) {
        let viewport = self.editors.viewport;
        let font_size = self.setting("editor.fontSize").as_f64().unwrap_or(12.) as f32 * self.editor_zoom.multiplier();
        layout(font_size, viewport.line_height, self.setting("editor.codeLensFontSize").as_f64().unwrap_or(0.) as f32)
    }

    /// `onModelChange` and `_onModelChange`'s debounced update: asks again when the model version or a provider's
    /// `onDidChangeCodeLenses` moved.
    pub(in crate::workbench) fn update_code_lenses(&mut self, cx: &mut Context<Self>) {
        let enabled = self.setting("editor.codeLens") != json!(false);
        let event = self.code_lens_event;
        let Some(editor) = self.editors.active() else { return };
        if !enabled {
            if !editor.code_lenses.groups.is_empty() {
                let editor = self.editors.active_mut().expect("found above");
                clear_groups(editor);
            }
            return;
        }
        let version = editor.version_id;
        if editor.code_lenses.version == Some(version) && editor.code_lenses.event == event {
            return;
        }
        let uri = editor.document.uri();
        let model_id = editor.model_id;
        let candidate = Candidate { uri: &uri, language: &editor.document.language_id };
        let calls: Vec<_> = self
            .language_features
            .ordered("$registerCodeLensSupport", &candidate)
            .iter()
            .map(|registration| (registration.handle, self.host.call_cancellable("ExtHostLanguageFeatures", "$provideCodeLenses", vec![json!(registration.handle), uri.clone(), json!({})])))
            .collect();
        let editor = self.editors.active_mut().expect("found above");
        editor.code_lenses.version = Some(version);
        editor.code_lenses.event = event;
        cx.spawn(async move |workbench, cx| {
            let mut lists = Vec::new();
            let mut lenses = Vec::new();
            for (rank, (handle, call)) in calls.into_iter().enumerate() {
                let Ok(Ok(list)) = call.await else { continue };
                if !list["cacheId"].is_null() {
                    lists.push((handle, list["cacheId"].clone()));
                }
                lenses.extend(list["lenses"].as_array().into_iter().flatten().map(|symbol| (rank, Lens { handle, symbol: symbol.clone() })));
            }
            let groups = group_lenses(lenses);
            let unresolved: Vec<(usize, usize, Lens)> = groups.iter().enumerate().flat_map(|(group, (_, lenses))| lenses.iter().enumerate().filter(|(_, lens)| lens.symbol["command"].is_null()).map(move |(index, lens)| (group, index, lens.clone()))).collect();
            let Ok(resolves) = workbench.update(cx, |workbench, cx| {
                workbench.show_code_lenses(model_id, version, lists, groups, cx);
                // shortcut: every unresolved lens resolves at once, upstream only those in the viewport.
                unresolved.into_iter().map(|(group, index, lens)| (group, index, workbench.host.call_cancellable("ExtHostLanguageFeatures", "$resolveCodeLens", vec![json!(lens.handle), lens.symbol]))).collect::<Vec<_>>()
            }) else {
                return;
            };
            for (group, index, call) in resolves {
                let Ok(Ok(symbol)) = call.await else { continue };
                let _ = workbench.update(cx, |workbench, cx| {
                    let Some(editor) = workbench.editors.editors.iter_mut().chain(workbench.editors.models.iter_mut()).find(|editor| editor.model_id == model_id) else { return };
                    if editor.code_lenses.version == Some(version)
                        && let Some(lens) = editor.code_lenses.groups.get_mut(group).and_then(|group| group.2.get_mut(index))
                    {
                        lens.symbol = symbol;
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }

    fn show_code_lenses(&mut self, model_id: u32, version: u32, lists: Vec<(u64, Value)>, groups: Vec<(usize, Vec<Lens>)>, cx: &mut Context<Self>) {
        let (_, height) = self.code_lens_layout();
        let Some(editor) = self.editors.editors.iter_mut().chain(self.editors.models.iter_mut()).find(|editor| editor.model_id == model_id) else { return };
        if editor.code_lenses.version != Some(version) {
            return;
        }
        for (handle, cache_id) in std::mem::replace(&mut editor.code_lenses.lists, lists) {
            drop(self.host.call("ExtHostLanguageFeatures", "$releaseCodeLenses", vec![json!(handle), cache_id]));
        }
        clear_groups(editor);
        for (line, lenses) in groups {
            if line >= editor.document.lines.len() {
                continue;
            }
            let anchor = line.checked_sub(1).map(|above| Position::new(above, 0));
            let zone = editor.add_view_zone(anchor, height);
            editor.code_lenses.groups.push((line, zone, lenses));
        }
        cx.notify();
    }

    /// The `codelens-decoration` of a view zone, if it is a lens group's: the commands separated by `\u{a0}|\u{a0}`,
    /// from the line's first non-whitespace column.
    pub(super) fn code_lens_zone(&self, editor: &OpenEditor, zone: u32, content_left: f32, cx: &mut Context<Self>) -> Option<Div> {
        let (line, _, lenses) = editor.code_lenses.groups.iter().find(|group| group.1 == zone)?;
        let (font_size, height) = self.code_lens_layout();
        let foreground = self.color("editorCodeLens.foreground");
        let link = self.color("editorLink.activeForeground");
        let text = &editor.document.lines[*line];
        let indent = crate::editor::visible_column(text, text.chars().take_while(|ch| ch.is_whitespace()).count(), editor.document.indentation.tab_size);
        let left = content_left + indent as f32 * self.editors.viewport.char_width - editor.scroll.left;
        // codelensWidget.css: an empty `editor.codeLensFontFamily` leaves the workbench's UI font.
        let family = self.setting("editor.codeLensFontFamily").as_str().filter(|family| !family.is_empty()).unwrap_or(".SystemUIFont").to_string();
        let mut row = div().absolute().top_0().left(px(left)).h(px(height)).flex().items_center().whitespace_nowrap().font_family(family).text_size(px(font_size)).text_color(foreground).pr(px(font_size * 0.5));
        let commands: Vec<&Value> = lenses.iter().map(|lens| &lens.symbol["command"]).filter(|command| command.is_object()).collect();
        if commands.is_empty() {
            return Some(div().relative().flex_none().h(px(height)).child(row.child("no commands")));
        }
        for (index, command) in commands.iter().enumerate() {
            let title = command["title"].as_str().unwrap_or_default().trim().to_string();
            let label = self.highlighted_label(&title, &[], foreground, font_size, foreground);
            match command["id"].as_str().filter(|id| !id.is_empty()) {
                Some(id) => {
                    let (id, args) = (id.to_string(), command["arguments"].as_array().cloned().unwrap_or_default());
                    row = row.child(
                        div()
                            .id(gpui::SharedString::from(format!("codelens-{zone}-{index}")))
                            .debug_selector(move || format!("codelens-{index}"))
                            .cursor_pointer()
                            .hover(move |item| item.text_color(link))
                            .child(label)
                            .on_mouse_down(MouseButton::Left, cx.listener(move |workbench, _: &MouseDownEvent, window, cx| {
                                cx.stop_propagation();
                                drop(workbench.execute_command(&id, args.clone(), window, cx));
                            })),
                    );
                }
                None => row = row.child(label),
            }
            if index + 1 < commands.len() {
                row = row.child("\u{a0}|\u{a0}");
            }
        }
        Some(div().relative().flex_none().h(px(height)).child(row))
    }
}

fn clear_groups(editor: &mut OpenEditor) {
    for (_, zone, _) in std::mem::take(&mut editor.code_lenses.groups) {
        editor.remove_view_zone(zone);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lens(handle: u64, line: u64, column: u64) -> (usize, Lens) {
        (handle as usize, Lens { handle, symbol: json!({ "range": { "startLineNumber": line, "startColumn": column }, "command": { "id": "x", "title": "t" } }) })
    }

    #[test]
    fn lenses_group_by_line_in_provider_and_column_order() {
        let groups = group_lenses(vec![lens(1, 5, 3), lens(0, 5, 9), lens(0, 2, 1), lens(1, 5, 1)]);
        let shape: Vec<(usize, Vec<(u64, u64)>)> = groups.into_iter().map(|(line, lenses)| (line, lenses.iter().map(|lens| (lens.handle, lens.symbol["range"]["startColumn"].as_u64().unwrap())).collect())).collect();
        assert_eq!(shape, vec![(1, vec![(0, 1)]), (4, vec![(0, 9), (1, 1), (1, 3)])]);
    }

    #[test]
    fn layout_follows_the_font_size() {
        // 12px font, 18px lines: 10px lenses (12 * 0.9 truncated) 15px high.
        assert_eq!(layout(12., 18., 0.), (10., 15.));
        assert_eq!(layout(14., 14., 8.), (8., 10.));
    }

    #[gpui::test]
    fn a_provider_registered_after_the_editor_opened_draws_its_lenses(cx: &mut gpui::TestAppContext) {
        let app_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap();
        let themes = exthost::scan::scan_extension(&app_root.join("extensions/theme-defaults"), true).unwrap();
        let theme = crate::theme::load_theme(&[themes], "Dark Modern").unwrap();
        let dir = std::env::temp_dir().join(format!("code-gpui-code-lens-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "one\ntwo").unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let settings = std::sync::Arc::new(std::sync::Mutex::new(config::service::Settings::load(config::registry::Registry::with_extensions(&[]), dir.join("User"), None)));
        let (workbench, cx) = cx.add_window_view(|_, cx| Workbench::new(theme, app_root.clone(), None, crate::languages::Languages::new(&[]), settings, crate::host_sync::HostSync::new(sender), cx));
        let uri = json!({ "scheme": "file", "path": dir.join("a.txt").to_string_lossy() });
        workbench.update_in(cx, |workbench, window, cx| drop(workbench.execute_command("_workbench.open", vec![uri], window, cx)));
        cx.run_until_parked();
        assert!(!receiver.try_iter().any(|message| message.method == "$provideCodeLenses"));

        let register = exthost::session::ForwardedRequest { req: 1, proxy: "MainThreadLanguageFeatures", method: "$registerCodeLensSupport".into(), args: vec![json!(7), json!([{ "scheme": "file" }]), json!(null)] };
        workbench.update_in(cx, |workbench, window, cx| drop(workbench.handle_host_request(register, window, cx)));
        cx.run_until_parked();
        let call = receiver.try_iter().find(|message| message.method == "$provideCodeLenses").expect("codelensController.ts asks on the registry's onDidChange");
        let lens = json!({ "range": { "startLineNumber": 2, "startColumn": 1, "endLineNumber": 2, "endColumn": 1 }, "command": { "id": "x", "title": "Accept" } });
        drop(call.reply.unwrap().send(Ok(json!({ "lenses": [lens] }))));
        cx.run_until_parked();
        let bounds = cx.debug_bounds("codelens-0").expect("the lens is drawn");
        assert!(bounds.size.width > gpui::px(0.) && bounds.size.height > gpui::px(0.));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
