//! viewsWelcomeContribution.ts and viewPane.ts `ViewWelcomeController`: the welcome content of the Explorer's
//! empty view and the Source Control view, registered by the workbench and by `contributes.viewsWelcome`.

use std::collections::HashMap;
use std::ops::Range;

use exthost::scan::ScannedExtension;
use gpui::{div, prelude::FluentBuilder, px, Context, Div, InteractiveElement, InteractiveText, ParentElement, StatefulInteractiveElement, Styled, StyledText, Window};
use serde_json::Value;

use super::opener::{OpenOptions, Target};
use super::Workbench;
use crate::contextkey::Expr;
use crate::notifications::{parse_linked_text, Node};

/// explorerViewlet.ts `EmptyView.ID`.
pub const EXPLORER_EMPTY_VIEW: &str = "workbench.explorer.emptyView";
/// scm.ts `VIEW_PANE_ID`.
pub const SCM_VIEW: &str = "workbench.scm";

/// `IViewContentDescriptor`; a `when` of "default" shows only while no other content of the view does.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewContent {
    pub content: String,
    pub when: Option<String>,
    pub precondition: Option<String>,
    pub group: Option<String>,
    pub order: Option<f64>,
}

const OPEN_FOLDER: &str = "[Open Folder](command:workbench.action.files.openFolder)";

/// explorerViewlet.ts and scm.contribution.ts registrations for a local desktop window (the remote one is left out).
fn workbench_contents(view: &str) -> Vec<ViewContent> {
    let content = |content: String, when: &str, group: Option<&str>| ViewContent { content, when: Some(when.into()), precondition: None, group: group.map(str::to_owned), order: group.map(|_| 1.) };
    match view {
        EXPLORER_EMPTY_VIEW => vec![
            content("You have not yet added a folder to the workspace.\n[Open Folder](command:workbench.action.addRootFolder)".into(), "workbenchState == 'workspace' && openFolderWorkspaceSupport", Some("2_open")),
            content(
                "You have not yet opened a folder.\n[Open Folder](command:workbench.action.files.openFolderViaWorkspace)\n[Open Recent](command:workbench.action.openRecent)".into(),
                "workbenchState == 'workspace' && !openFolderWorkspaceSupport",
                Some("2_open"),
            ),
            content(
                format!("You have not yet opened a folder.\n{OPEN_FOLDER}\nOpening a folder will close all currently open editors. To keep them open, [add a folder](command:workbench.action.addRootFolder) instead."),
                "editorIsOpen && (workbenchState != 'workspace' && remoteName == '' || workbenchState != 'workspace' && isWeb)",
                Some("2_open"),
            ),
            content(format!("You have not yet opened a folder.\n{OPEN_FOLDER}"), "!editorIsOpen && (workbenchState != 'workspace' && remoteName == '' || workbenchState != 'workspace' && isWeb)", Some("2_open")),
        ],
        SCM_VIEW => vec![
            content("No source control providers registered.".into(), "default", None),
            content("None of the registered source control providers work in Restricted Mode.".into(), "scm.providerCount == 0 && isWorkspaceTrustEnabled && !isWorkspaceTrusted", None),
            content("[Manage Workspace Trust](command:workbench.trust.manage)".into(), "scm.providerCount == 0 && isWorkspaceTrustEnabled && !isWorkspaceTrusted", None),
        ],
        _ => Vec::new(),
    }
}

/// viewsWelcomeExtensionPoint.ts `ViewIdentifierMap`.
fn view_id(view: &str) -> &str {
    match view {
        "explorer" => EXPLORER_EMPTY_VIEW,
        "debug" => "workbench.debug.welcome",
        "scm" => SCM_VIEW,
        "testing" => "workbench.view.testing",
        view => view,
    }
}

/// The view's contents in registration order: the workbench's, then each extension's `viewsWelcome` entries.
pub fn view_contents(view: &str, extensions: &[ScannedExtension]) -> Vec<ViewContent> {
    let mut contents = workbench_contents(view);
    for extension in extensions {
        let proposals = extension.description["enabledApiProposals"].as_array();
        let grouping = proposals.is_some_and(|proposals| proposals.iter().any(|proposal| proposal.as_str().is_some_and(|name| name.split('@').next() == Some("contribViewsWelcome"))));
        for welcome in extension.description["contributes"]["viewsWelcome"].as_array().into_iter().flatten() {
            let (Some(target), Some(content)) = (welcome["view"].as_str(), welcome["contents"].as_str()) else { continue };
            if view_id(target) != view {
                continue;
            }
            // parseGroupAndOrder: `group` needs the contribViewsWelcome proposal; "group@order".
            let (group, order) = match welcome["group"].as_str().filter(|_| grouping) {
                Some(group) => match group.rfind('@').filter(|index| *index > 0) {
                    Some(index) => (Some(group[..index].to_owned()), group[index + 1..].parse::<f64>().ok().filter(|order| *order != 0.)),
                    None => (Some(group.to_owned()), None),
                },
                None => (None, None),
            };
            let string = |key: &str| welcome[key].as_str().map(str::to_owned);
            contents.push(ViewContent { content: content.to_owned(), when: string("when"), precondition: string("enablement"), group, order });
        }
    }
    // views.ts `compareViewContentDescriptors`, a stable sort.
    contents.sort_by(|a, b| {
        let group = |content: &ViewContent| content.group.clone().unwrap_or_else(|| "9_more".into());
        group(a).cmp(&group(b)).then(a.order.unwrap_or(5.).total_cmp(&b.order.unwrap_or(5.)))
    });
    contents
}

/// `ViewWelcomeController.getContentDescriptors`: the contents whose `when` holds, else the default one.
pub fn visible_contents(contents: &[ViewContent], context: &HashMap<String, Value>) -> Vec<ViewContent> {
    let holds = |when: &Option<String>| when.as_deref().and_then(Expr::deserialize).is_none_or(|when| when.evaluate(context));
    let visible: Vec<ViewContent> = contents.iter().filter(|content| content.when.as_deref() != Some("default") && holds(&content.when)).cloned().collect();
    if visible.is_empty() {
        return contents.iter().filter(|content| content.when.as_deref() == Some("default")).take(1).cloned().collect();
    }
    visible
}

impl Workbench {
    /// The view's `.welcome-view-content`, or `None` when no content shows.
    pub(super) fn view_welcome(&self, view: &str, window: &Window, cx: &mut Context<Self>) -> Option<Div> {
        let context = self.key_context(window);
        let contents = visible_contents(&view_contents(view, &self.extensions), &context);
        if contents.is_empty() {
            return None;
        }
        let mut body = div().px(px(20.)).pb(px(13.)).flex().flex_col().items_center().text_size(px(13.)).line_height(px(18.2)).text_color(self.foreground_or("sideBar.foreground"));
        let mut index = 0;
        for content in contents {
            let enabled = content.precondition.as_deref().and_then(Expr::deserialize).is_none_or(|precondition| precondition.evaluate(&context));
            for line in content.content.split('\n').map(str::trim).filter(|line| !line.is_empty()) {
                index += 1;
                let nodes = parse_linked_text(line);
                body = match nodes.as_slice() {
                    [Node::Link { label, href, .. }] => body.child(self.view_welcome_button(index, label, href, enabled, cx)),
                    _ => body.child(div().w_full().mt(px(13.)).child(self.view_welcome_paragraph(index, &nodes, enabled, cx))),
                };
            }
        }
        Some(body)
    }

    /// A `.button-container` with its `monaco-text-button`, at most 300px wide; button.css dims a disabled one.
    fn view_welcome_button(&self, index: usize, label: &str, href: &str, enabled: bool, cx: &mut Context<Self>) -> Div {
        let hover = self.color("button.hoverBackground");
        let href = href.to_owned();
        div().w_full().max_w(px(300.)).mt(px(13.)).child(
            div()
                .id(("view-welcome-button", index))
                .h(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .text_size(px(12.))
                .line_height(px(16.))
                .bg(self.color("button.background"))
                .text_color(self.color("button.foreground"))
                .when_some(self.theme.color("button.border"), |button, border| button.border_1().border_color(border))
                .child(label.to_owned())
                .when(!enabled, |button| button.opacity(0.4))
                .when(enabled, |button| {
                    button.cursor_pointer().hover(move |button| button.bg(hover)).on_click(cx.listener(move |workbench, _, window, cx| {
                        drop(workbench.open_link(Target::Text(href.clone()), OpenOptions { allow_commands: true, ..Default::default() }, window, cx));
                    }))
                }),
        )
    }

    /// A `<p>` of text and `Link`s; a disabled command link takes no click.
    fn view_welcome_paragraph(&self, index: usize, nodes: &[Node], enabled: bool, cx: &mut Context<Self>) -> InteractiveText {
        let mut text = String::new();
        let mut links: Vec<(Range<usize>, String)> = Vec::new();
        for node in nodes {
            match node {
                Node::Text(value) => text.push_str(value),
                Node::Link { label, href, .. } => {
                    links.push((text.len()..text.len() + label.len(), href.clone()));
                    text.push_str(label);
                }
            }
        }
        let clickable: Vec<(Range<usize>, String)> = links.iter().filter(|(_, href)| enabled || !href.starts_with("command:")).cloned().collect();
        let link_color = self.color("textLink.foreground");
        let disabled_color = self.color("disabledForeground");
        let highlights = links.iter().map(|(range, href)| {
            let color = if enabled || !href.starts_with("command:") { link_color } else { disabled_color };
            (range.clone(), gpui::HighlightStyle { color: Some(color.into()), ..Default::default() })
        });
        let weak = cx.entity().downgrade();
        let ranges = clickable.iter().map(|(range, _)| range.clone()).collect();
        InteractiveText::new(("view-welcome-text", index), StyledText::new(text).with_highlights(highlights)).on_click(ranges, move |clicked, window, cx| {
            let href = clickable[clicked].1.clone();
            let _ = weak.update(cx, |workbench, cx| drop(workbench.open_link(Target::Text(href), OpenOptions { allow_commands: true, ..Default::default() }, window, cx)));
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn extension(proposals: Value, welcomes: Value) -> ScannedExtension {
        ScannedExtension { id: "vscode.git".into(), location: "/x".into(), description: json!({ "enabledApiProposals": proposals, "contributes": { "viewsWelcome": welcomes } }) }
    }

    fn context(pairs: &[(&str, Value)]) -> HashMap<String, Value> {
        pairs.iter().map(|(key, value)| (key.to_string(), value.clone())).collect()
    }

    #[test]
    fn extension_contents_map_the_view_and_sort_by_group_and_order() {
        let git = extension(
            json!(["contribViewsWelcome"]),
            json!([
                { "view": "explorer", "contents": "learn", "group": "5_scm@10" },
                { "view": "explorer", "contents": "clone", "group": "5_scm@1" },
                { "view": "scm", "contents": "other view" },
            ]),
        );
        let contents: Vec<String> = view_contents(EXPLORER_EMPTY_VIEW, &[git]).into_iter().map(|content| content.content).collect();
        assert_eq!(contents.len(), 6);
        assert!(contents[..4].iter().all(|content| content.contains("folder")), "2_open sorts before 5_scm: {contents:?}");
        assert_eq!(contents[4..], ["clone".to_string(), "learn".to_string()]);
    }

    #[test]
    fn group_needs_the_proposal() {
        let contents = view_contents("custom", &[extension(json!([]), json!([{ "view": "custom", "contents": "a", "group": "1_first@2" }]))]);
        assert_eq!((contents[0].group.clone(), contents[0].order), (None, None));
    }

    #[test]
    fn default_shows_only_without_other_content() {
        let contents = view_contents(SCM_VIEW, &[extension(json!([]), json!([{ "view": "scm", "contents": "git missing", "when": "git.missing" }]))]);
        let shown = |pairs: &[(&str, Value)]| visible_contents(&contents, &context(pairs)).into_iter().map(|content| content.content).collect::<Vec<_>>();
        assert_eq!(shown(&[("scm.providerCount", json!(0))]), ["No source control providers registered."]);
        assert_eq!(shown(&[("git.missing", json!(true))]), ["git missing"]);
    }

    #[test]
    fn explorer_help_follows_open_editors() {
        let contents = view_contents(EXPLORER_EMPTY_VIEW, &[]);
        let shown = |open: bool| visible_contents(&contents, &context(&[("editorIsOpen", json!(open)), ("workbenchState", json!("empty")), ("remoteName", json!("")), ("openFolderWorkspaceSupport", json!(true))]));
        assert!(shown(false)[0].content.ends_with(OPEN_FOLDER));
        assert!(shown(true)[0].content.contains("add a folder"));
        assert_eq!(shown(true).len(), 1);
    }
}
