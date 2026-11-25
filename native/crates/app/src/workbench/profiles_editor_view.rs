//! userDataProfilesEditor.ts: the Profiles editor in the modal editor part. The sidebar has the New Profile
//! button (Import Profile... in its dropdown) and the profiles; the contents view shows the selected profile
//! (Use for Current Window, Use for New Windows, its contents and the folders and workspaces using it), or the form
//! of a new profile (name, the profile to copy from, Create).

use std::collections::HashSet;

use gpui::{AnyElement, Context, FocusHandle, FontWeight, MouseButton, PathPromptOptions, Window, div, prelude::*, px};
use serde_json::{Value, json};
use storage::profiles::{Options, Profile};

use super::Workbench;
use super::input_box_view::{InputId, InputLook, InputView};
use super::keybindings_editor_view::ModalHeader;
use crate::commands::Outcome;
use crate::input_box::InputBox;
use crate::menus::MenuAction;
use crate::profile_template::{self, Flags, PROFILE_EXTENSION};

/// `CONFIG_NEW_WINDOW_PROFILE`.
pub const NEW_WINDOW_PROFILE_SETTING: &str = "window.newWindowProfile";
/// The sidebar's width, the reference's at 1024x768.
const SIDEBAR_WIDTH: f32 = 268.;
const ROW_HEIGHT: f32 = 22.;
/// `ProfileResourceType` in the Contents tree order, with their titles.
const RESOURCES: [(&str, &str); 6] = [("settings", "Settings"), ("keybindings", "Keyboard Shortcuts"), ("tasks", "Tasks"), ("mcp", "MCP Servers"), ("snippets", "Snippets"), ("extensions", "Extensions")];

#[derive(Default)]
pub struct ProfilesEditor {
    pub open: bool,
    /// The selected profile's id; `None` while the new profile form is selected.
    pub selected: Option<String>,
    /// `NewProfileElement`: shown while a new profile is being made.
    pub new_profile: Option<NewProfile>,
    pub name: InputBox,
    pub name_view: InputView,
    pub name_focus: Option<FocusHandle>,
    pub focus: Option<FocusHandle>,
    expanded: HashSet<&'static str>,
}

/// The form of a new profile: copied from a profile, or from an imported template.
pub struct NewProfile {
    pub copy_from: Option<String>,
    pub template: Option<Value>,
    pub error: Option<String>,
}

impl Workbench {
    pub(super) fn register_profiles_editor_commands(&mut self) {
        self.commands.register("workbench.profiles.actions.manageProfiles", |workbench, _, window, cx| {
            workbench.open_profiles_editor(window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.profiles.actions.createProfile", |workbench, _, window, cx| {
            workbench.open_profiles_editor(window, cx);
            workbench.start_new_profile(None, None, window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.profiles.actions.createFromCurrentProfile", |workbench, _, window, cx| {
            workbench.open_profiles_editor(window, cx);
            let current = workbench.user_data_profiles.state.as_ref().map(|state| state.current.id.clone());
            workbench.start_new_profile(current, None, window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.profiles.actions.exportProfile", |workbench, _, window, cx| {
            workbench.open_profiles_editor(window, cx);
            let Some(current) = workbench.user_data_profiles.state.as_ref().map(|state| state.current.clone()) else { return Outcome::done() };
            workbench.user_data_profiles.editor.selected = Some(current.id.clone());
            workbench.export_profile(current, window, cx);
            Outcome::done()
        });
        self.commands.register("workbench.profiles.actions.importProfile", |workbench, _, window, cx| {
            workbench.open_profiles_editor(window, cx);
            workbench.import_profile(window, cx);
            Outcome::done()
        });
    }

    pub(super) fn open_profiles_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_settings_editor(window, cx);
        self.close_keybindings_editor(window, cx);
        let editor = &mut self.user_data_profiles.editor;
        if editor.open {
            return;
        }
        editor.open = true;
        let current = self.user_data_profiles.state.as_ref().map(|state| state.current.clone());
        let editor = &mut self.user_data_profiles.editor;
        editor.selected = current.as_ref().map(|profile| profile.id.clone());
        editor.name = InputBox::new(current.map(|profile| profile.name).unwrap_or_default());
        let focus = editor.focus.get_or_insert_with(|| cx.focus_handle()).clone();
        editor.name_focus.get_or_insert_with(|| cx.focus_handle());
        window.focus(&focus, cx);
        cx.notify();
    }

    pub(super) fn close_profiles_editor(&mut self, cx: &mut Context<Self>) {
        let editor = &mut self.user_data_profiles.editor;
        editor.open = false;
        editor.new_profile = None;
        cx.notify();
    }

    /// `createNewProfile(copyFrom)`: the new profile form, named after the template or "Untitled".
    fn start_new_profile(&mut self, copy_from: Option<String>, template: Option<Value>, window: &mut Window, cx: &mut Context<Self>) {
        let name = template.as_ref().and_then(|template| template["name"].as_str()).unwrap_or("Untitled").to_owned();
        let editor = &mut self.user_data_profiles.editor;
        editor.new_profile = Some(NewProfile { copy_from, template, error: None });
        editor.selected = None;
        editor.name = InputBox::new(name);
        editor.name.select_all();
        editor.name_view.reset();
        if let Some(focus) = editor.name_focus.clone() {
            window.focus(&focus, cx);
        }
        self.blink_caret(InputId::ProfileName, window, cx);
        cx.notify();
    }

    /// The Name input: the new profile's name, or a rename of the selected profile once the name is valid
    /// (`ProfileNameRenderer`'s validation, then `updateProfile`).
    pub(super) fn edit_profile_name(&mut self, edit: impl FnOnce(&mut InputBox), cx: &mut Context<Self>) {
        let editor = &mut self.user_data_profiles.editor;
        edit(&mut editor.name);
        if let Some(new_profile) = &mut editor.new_profile {
            new_profile.error = None;
            return cx.notify();
        }
        let name = editor.name.value.trim().to_owned();
        let (Some(id), Some(state)) = (editor.selected.clone(), self.user_data_profiles.state.as_mut()) else { return };
        let taken = state.profiles.profiles().iter().any(|profile| profile.name == name && profile.id != id);
        if !name.is_empty() && !taken {
            let update = storage::profiles::UpdateOptions { name: Some(name), ..Default::default() };
            if let Ok((renamed, _)) = state.edit(|profiles, storage| profiles.update_profile(&id, update, storage)) {
                if state.current.id == renamed.id {
                    state.current = renamed;
                }
                self.profiles_changed(cx);
            }
        }
        cx.notify();
    }

    fn select_profile(&mut self, profile: &Profile, cx: &mut Context<Self>) {
        let editor = &mut self.user_data_profiles.editor;
        editor.selected = Some(profile.id.clone());
        editor.new_profile = None;
        editor.name = InputBox::new(profile.name.clone());
        editor.name_view.reset();
        cx.notify();
    }

    /// `NewProfileElement.saveProfile`: creates the profile, copies or applies its contents and switches to it.
    fn create_new_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(new_profile) = &self.user_data_profiles.editor.new_profile else { return };
        let name = self.user_data_profiles.editor.name.value.trim().to_owned();
        let Some(state) = self.user_data_profiles.state.as_ref() else { return };
        if name.is_empty() {
            return self.set_new_profile_error("Profile name is required and must be a non-empty value.", cx);
        }
        if state.profiles.profiles().iter().any(|profile| profile.name == name) {
            return self.set_new_profile_error(&format!("Profile with name {name} already exists."), cx);
        }
        let copy_from = new_profile.copy_from.clone().and_then(|id| state.profiles.profiles().into_iter().find(|profile| profile.id == id));
        let icon = copy_from.as_ref().and_then(|profile| profile.icon.clone());
        let template = match (&new_profile.template, &copy_from) {
            (Some(template), _) => Some(template.clone()),
            (None, Some(from)) => Some(self.profile_template(from, &name)),
            (None, None) => None,
        };
        let state = self.user_data_profiles.state.as_mut().expect("checked above");
        let created = state.edit(|profiles, storage| profiles.create_named_profile(&name, Options { icon, ..Options::default() }, None, storage));
        let profile = match created {
            Ok((profile, _)) => profile,
            Err(error) => return self.set_new_profile_error(&error, cx),
        };
        if let Some(template) = template {
            match self.apply_profile_template(&template, &profile, cx) {
                Ok(()) => {}
                Err(error) => self.notify_error(error),
            }
        }
        self.user_data_profiles.editor.new_profile = None;
        self.user_data_profiles.editor.selected = Some(profile.id.clone());
        self.profiles_changed(cx);
        self.switch_profile(&profile, window, cx);
    }

    fn set_new_profile_error(&mut self, error: &str, cx: &mut Context<Self>) {
        if let Some(new_profile) = &mut self.user_data_profiles.editor.new_profile {
            new_profile.error = Some(error.to_owned());
        }
        cx.notify();
    }

    /// The template of `profile`: its files, its user global state and its extensions (`getLocalExtensions`).
    fn profile_template(&self, profile: &Profile, name: &str) -> Value {
        let settings = self.settings.lock().unwrap();
        let global_state = match self.user_data_profiles.state.as_ref().is_some_and(|state| state.current.id == profile.id) {
            true => None,
            false => Some(storage::Storage::open(&profile.global_storage_home.join("state.vscdb"))),
        };
        let current = self.profile_storage.lock().unwrap();
        let global_state = global_state.as_ref().unwrap_or(&current);
        let extensions = self.profile_extensions(profile);
        profile_template::export_template(profile, name, Flags::ALL, &settings.registry, &settings.configuration, global_state, &extensions)
    }

    /// `ExtensionsResource.getLocalExtensions`: the profile's gallery extensions, and the built-ins disabled in the
    /// current profile.
    fn profile_extensions(&self, profile: &Profile) -> Vec<Value> {
        let Some(user) = self.extensions_viewlet.user.as_ref() else { return Vec::new() };
        let user = exthost::extension_management::UserExtensions { extensions_file: profile.extensions_resource.clone(), ..user.clone_for_profile() };
        let mut extensions = Vec::new();
        for extension in user.scan().unwrap_or_default() {
            let Some(uuid) = extension.description["uuid"].as_str() else { continue };
            let mut entry = json!({ "identifier": { "id": extension.id.to_lowercase(), "uuid": uuid }, "displayName": extension.description["displayName"] });
            if extension.description["preRelease"] == true {
                entry["preRelease"] = json!(true);
            }
            extensions.push(entry);
        }
        let disabled = self.local_extensions().into_iter().filter(|extension| extension.is_builtin() && !extension.enablement.is_enabled());
        for extension in disabled {
            extensions.push(json!({ "identifier": { "id": extension.identifier().id.to_lowercase() }, "displayName": extension.display_name(), "disabled": true }));
        }
        extensions
    }

    /// `applyProfileTemplate` on a new profile; the listed gallery extensions are installed once the window has
    /// switched to it (`ExtensionsResource.apply`).
    fn apply_profile_template(&mut self, template: &Value, profile: &Profile, cx: &mut Context<Self>) -> Result<(), String> {
        let mut global_state = storage::Storage::open(&profile.global_storage_home.join("state.vscdb"));
        let settings = self.settings.lock().unwrap();
        let extensions = profile_template::apply_template(template, profile, &settings.registry, &settings.configuration, &mut global_state)?;
        drop(settings);
        let to_install: Vec<String> = extensions.iter().filter(|extension| extension["disabled"] != true).filter_map(|extension| extension["identifier"]["id"].as_str().map(str::to_owned)).collect();
        self.user_data_profiles.pending_installs = to_install;
        cx.notify();
        Ok(())
    }

    /// `exportProfile`: the template saved to a `.code-profile` file (FileUserDataProfileContentHandler).
    fn export_profile(&mut self, profile: Profile, window: &mut Window, cx: &mut Context<Self>) {
        let template = self.profile_template(&profile, &profile.name);
        let file_name = format!("{}.{PROFILE_EXTENSION}", profile.name.replace('/', "-"));
        let picked = cx.prompt_for_new_path(&self.default_file_path(), Some(&file_name));
        cx.spawn_in(window, async move |workbench, cx| {
            let Ok(Ok(Some(path))) = picked.await else { return };
            let written = std::fs::write(&path, template.to_string());
            let _ = workbench.update(cx, |workbench, cx| {
                match written {
                    Ok(()) => drop(workbench.prompt_notification(crate::notifications::Severity::Info, format!("Profile '{}' was exported successfully.", profile.name), &[])),
                    Err(error) => workbench.notify_error(format!("{}: {error}", path.display())),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// `importProfile` from a file: "Select Profile Template File", then the new profile form with its contents.
    fn import_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Select Profile Template File".into()) });
        cx.spawn_in(window, async move |workbench, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let template = std::fs::read_to_string(&path).map_err(|error| error.to_string()).and_then(|content| profile_template::parse_template(&content));
            let _ = workbench.update_in(cx, |workbench, window, cx| match template {
                Ok(template) => workbench.start_new_profile(None, Some(template), window, cx),
                Err(error) => workbench.notify_error(error),
            });
        })
        .detach();
    }

    /// `toggleNewWindowProfile`: `window.newWindowProfile` names the profile, or is removed.
    fn toggle_new_window_profile(&mut self, profile: &Profile, cx: &mut Context<Self>) {
        let mut settings = self.settings.lock().unwrap();
        let current = settings.get_value(Some(NEW_WINDOW_PROFILE_SETTING), &Default::default());
        let value = (current.as_ref().and_then(Value::as_str) != Some(profile.name.as_str())).then(|| json!(profile.name));
        let written = settings.update_value(NEW_WINDOW_PROFILE_SETTING, value.as_ref(), config::service::Target::UserLocal, None, &[]);
        drop(settings);
        match written {
            Ok(change) => self.configuration_changed(&change, self.model_creation_options(), cx),
            Err(error) => self.notify_error(error),
        }
    }

    /// The `...` actions of a profile row: Export..., Delete, Open in New Window.
    fn show_profile_actions(&mut self, profile: Profile, anchor: gpui::Bounds<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let mut actions = Vec::new();
        let mut register = |workbench: &mut Workbench, id: &str, title: &str, run: Box<dyn Fn(&mut Workbench, &mut Window, &mut Context<Workbench>)>| {
            let id = format!("_profilesEditor.{id}");
            workbench.commands.register(id.clone(), move |workbench, _, window, cx| {
                run(workbench, window, cx);
                Outcome::done()
            });
            actions.push(MenuAction::Command { id, title: title.into(), enabled: true, checked: false, alt: None });
        };
        let export = profile.clone();
        register(self, "export", "Export...", Box::new(move |workbench, window, cx| workbench.export_profile(export.clone(), window, cx)));
        if !profile.is_default {
            let delete = profile.clone();
            register(self, "delete", "Delete", Box::new(move |workbench, window, cx| workbench.remove_profile(&delete, window, cx)));
        }
        let name = profile.name.clone();
        register(self, "openInNewWindow", "Open in New Window", Box::new(move |workbench, _, _| drop(workbench.open_window_with_profile_flags(vec!["--profile".into(), name.clone()]))));
        let context = self.key_context(window);
        self.show_context_menu_actions(actions, &context, anchor, Vec::new(), window, cx);
    }

    pub(super) fn profiles_editor_widget(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.user_data_profiles.editor.open {
            return None;
        }
        let header = ModalHeader { icon: "settings", title: "Profiles", json_title: "", json_command: "" };
        let focus = self.user_data_profiles.editor.focus.clone()?;
        Some(self.modal_editor_part(header, &focus, window, cx, |workbench, width, height, window, cx| workbench.profiles_editor_pane(width, height, window, cx)))
    }

    fn profiles_editor_pane(&mut self, width: f32, _height: f32, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(state) = &self.user_data_profiles.state else { return div().into_any_element() };
        let profiles = state.profiles.profiles();
        let current = state.current.clone();
        let sash = self.theme.color("profiles.sashBorder").or(self.theme.color("panel.border")).unwrap_or_default();
        let sidebar = self.profiles_sidebar(&profiles, &current, cx);
        let contents = match (&self.user_data_profiles.editor.new_profile, &self.user_data_profiles.editor.selected) {
            (Some(_), _) => self.new_profile_contents(&profiles, window, cx).into_any_element(),
            (None, Some(id)) => match profiles.iter().find(|profile| profile.id == *id) {
                Some(profile) => self.profile_contents(profile, &current, window, cx).into_any_element(),
                None => div().into_any_element(),
            },
            (None, None) => div().into_any_element(),
        };
        div()
            .flex_1()
            .min_h_0()
            .w(px(width))
            .flex()
            .pt(px(20.))
            .text_size(px(13.))
            .text_color(self.color("foreground"))
            .child(sidebar)
            .child(div().w(px(1.)).h_full().mt(px(35.)).bg(sash))
            .child(div().id("profiles-contents").flex_1().min_w_0().h_full().overflow_y_scroll().child(contents))
            .into_any_element()
    }

    fn profiles_sidebar(&self, profiles: &[Profile], current: &Profile, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let button_background = self.color("button.background");
        let button = div()
            .id("new-profile-button")
            .h(px(26.))
            .flex_1()
            .flex()
            .rounded(px(4.))
            .bg(button_background)
            .text_color(self.color("button.foreground"))
            .child(
                div()
                    .id("new-profile")
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|button| button.bg(self.color("button.hoverBackground")))
                    .on_click(cx.listener(|workbench, _, window, cx| workbench.start_new_profile(None, None, window, cx)))
                    .child("New Profile"),
            )
            .child(div().w(px(1.)).my(px(4.)).bg(self.color("button.separator")))
            .child(
                self.icon("chevron-down", 26., 16., self.color("button.foreground"))
                    .id("new-profile-dropdown")
                    .cursor_pointer()
                    .on_click(cx.listener(|workbench, event: &gpui::ClickEvent, window, cx| {
                        let id = "workbench.profiles.actions.importProfile".to_owned();
                        let actions = vec![MenuAction::Command { id, title: "Import Profile...".into(), enabled: true, checked: false, alt: None }];
                        let position = event.position();
                        let anchor = gpui::Bounds::new(position, gpui::size(px(1.), px(1.)));
                        let context = workbench.key_context(window);
                        workbench.show_context_menu_actions(actions, &context, anchor, Vec::new(), window, cx);
                    })),
            );
        let selected = self.user_data_profiles.editor.selected.clone();
        let new_profile = self.user_data_profiles.editor.new_profile.is_some().then(|| self.user_data_profiles.editor.name.value.clone());
        let mut rows: Vec<AnyElement> = Vec::new();
        for profile in profiles {
            let is_selected = selected.as_deref() == Some(profile.id.as_str());
            rows.push(self.profile_row(profile, profile.id == current.id, is_selected, cx).into_any_element());
        }
        if let Some(name) = new_profile {
            let row = div().h(px(ROW_HEIGHT)).flex().items_center().pl(px(20.)).bg(self.color("list.activeSelectionBackground")).child(self.icon(profile_icon(None), 22., 16., self.color("foreground"))).child(div().ml(px(5.)).italic().child(name));
            rows.push(row.into_any_element());
        }
        div()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .pl(px(20.))
            .child(div().h(px(40.)).flex().items_center().pl(px(18.)).pr(px(20.)).child(button))
            .child(div().mt(px(15.)).flex().flex_col().children(rows))
    }

    fn profile_row(&self, profile: &Profile, active: bool, selected: bool, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let foreground = if selected { self.foreground_or("list.activeSelectionForeground") } else { self.color("foreground") };
        let description = active.then(|| div().ml(px(2.)).flex().items_center().text_size(px(11.7)).opacity(0.7).child(self.icon("check", 16., 14., foreground)).child("Active"));
        let select = profile.clone();
        let menu_profile = profile.clone();
        let switch_to = profile.clone();
        let open_name = profile.name.clone();
        let actions = div()
            .flex_1()
            .flex()
            .justify_end()
            .mr(px(10.))
            .child(self.icon("empty-window", 22., 16., foreground).id(("profile-open-window", profile.id.len())).cursor_pointer().on_click(cx.listener(move |workbench, _, _, _| drop(workbench.open_window_with_profile_flags(vec!["--profile".into(), open_name.clone()])))))
            .when(!active, |actions| {
                actions.child(self.icon("window", 22., 16., foreground).id(("profile-use", profile.id.len())).cursor_pointer().on_click(cx.listener(move |workbench, _, window, cx| workbench.switch_profile(&switch_to, window, cx))))
            })
            .child(self.icon("ellipsis", 22., 16., foreground).id(("profile-actions", profile.id.len())).cursor_pointer().on_click(cx.listener(move |workbench, event: &gpui::ClickEvent, window, cx| {
                let anchor = gpui::Bounds::new(event.position(), gpui::size(px(1.), px(1.)));
                workbench.show_profile_actions(menu_profile.clone(), anchor, window, cx);
            })));
        div()
            .id(gpui::ElementId::Name(format!("profile-row-{}", profile.id).into()))
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .pl(px(20.))
            .cursor_pointer()
            .text_color(foreground)
            .when(selected, |row| row.bg(self.color("list.activeSelectionBackground")))
            .when(!selected, |row| row.hover(|row| row.bg(self.color("list.hoverBackground"))))
            .on_click(cx.listener(move |workbench, _, _, cx| workbench.select_profile(&select, cx)))
            .child(self.icon(profile_icon(profile.icon.as_deref()), 22., 16., foreground))
            .child(div().ml(px(5.)).truncate().child(profile.name.clone()))
            .children(description)
            .when(selected || active, |row| row.child(actions))
    }

    fn profile_name_input(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let editor = &self.user_data_profiles.editor;
        let focused = editor.name_focus.as_ref().is_some_and(|focus| focus.is_focused(window));
        let border = if focused { self.color("focusBorder") } else { self.theme.color("input.border").unwrap_or_default() };
        div().w(px(400.)).h(px(26.)).flex().items_center().px(px(6.)).border_1().border_color(border).bg(self.color("input.background")).child(self.input_text(InputId::ProfileName, InputLook { password: false, placeholder: Some("Profile Name".into()), focused }, cx))
    }

    fn profile_contents(&self, profile: &Profile, current: &Profile, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let new_window_profile = self.setting(NEW_WINDOW_PROFILE_SETTING);
        let is_new_window = new_window_profile.as_str() == Some(profile.name.as_str());
        let title = div()
            .ml(px(27.))
            .h(px(40.))
            .flex()
            .items_baseline()
            .child(div().text_size(px(26.)).font_weight(FontWeight::SEMIBOLD).child(profile.name.clone()))
            .when(profile.is_default, |title| title.child(div().ml(px(10.)).text_size(px(16.)).italic().child("Built-in")));
        let use_current = profile.clone();
        let use_for_current = (profile.id != current.id).then(|| {
            self.profile_row_container("Use for Current Window", self.profile_checkbox("profile-use-current", false, "Use this profile for the current window", cx, move |workbench, window, cx| workbench.switch_profile(&use_current, window, cx)))
        });
        let new_window = profile.clone();
        let use_for_new = self.profile_row_container(
            "Use for New Windows",
            self.profile_checkbox("profile-use-new", is_new_window, "Use this profile as the default for new windows", cx, move |workbench, _, cx| workbench.toggle_new_window_profile(&new_window, cx)),
        );
        let contents = self.profile_row_container("Contents", div().flex().flex_col().child(div().line_height(px(18.)).opacity(0.9).child("Browse contents of this profile")).child(self.profile_contents_tree(profile, cx)));
        let workspaces: Vec<String> = profile.workspaces.clone().unwrap_or_default();
        let description = if workspaces.is_empty() { "No folders or workspaces are using this profile" } else { "Following folders and workspaces are using this profile" };
        let header_background = self.color("keybindingTable.headerBackground");
        let mut table = div().mt(px(8.)).flex().flex_col().child(div().h(px(30.)).flex().items_center().bg(header_background).font_weight(FontWeight::SEMIBOLD).child(div().w(px(155.)).pl(px(35.)).child("Host")).child(div().child("Path")));
        for workspace in &workspaces {
            let path = crate::uri::percent_decode(workspace.strip_prefix("file://").unwrap_or(workspace));
            table = table.child(div().h(px(28.)).flex().items_center().italic().font_weight(FontWeight::SEMIBOLD).child(div().w(px(155.)).pl(px(35.)).child("Local")).child(div().flex_1().truncate().child(path)));
        }
        let add_folder_profile = profile.clone();
        let add_folder = self.text_button("profile-add-folder", "Add Folder", cx, move |workbench, window, cx| workbench.add_profile_folder(add_folder_profile.clone(), window, cx));
        let folders = self.profile_row_container("Folders & Workspaces", div().flex().flex_col().child(div().line_height(px(18.)).opacity(0.9).child(description)).child(table).child(div().mt(px(8.)).flex().child(add_folder)));
        let name = (!profile.is_default).then(|| self.profile_row_container("Name", self.profile_name_input(window, cx)));
        div().flex().flex_col().child(title).child(div().mt(px(15.)).ml(px(28.)).children(name).children(use_for_current).child(use_for_new).child(contents).child(folders))
    }

    fn new_profile_contents(&self, profiles: &[Profile], window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let editor = &self.user_data_profiles.editor;
        let new_profile = editor.new_profile.as_ref().expect("the new profile form is shown");
        let name = self.profile_name_input(window, cx);
        let error = new_profile.error.clone().map(|error| div().w(px(400.)).px(px(6.)).py(px(4.)).border_1().border_color(self.color("inputValidation.errorBorder")).bg(self.color("inputValidation.errorBackground")).child(error));
        let copy_from_name = match (&new_profile.template, &new_profile.copy_from) {
            (Some(template), _) => template["name"].as_str().unwrap_or_default().to_owned(),
            (None, Some(id)) => profiles.iter().find(|profile| profile.id == *id).map(|profile| profile.name.clone()).unwrap_or_default(),
            (None, None) => "None".to_owned(),
        };
        let copy_from = div()
            .id("profile-copy-from")
            .w(px(250.))
            .h(px(26.))
            .flex()
            .items_center()
            .justify_between()
            .px(px(6.))
            .border_1()
            .border_color(self.color("dropdown.border"))
            .bg(self.color("dropdown.background"))
            .cursor_pointer()
            .on_click(cx.listener(|workbench, event: &gpui::ClickEvent, window, cx| workbench.show_copy_from_menu(event.position(), window, cx)))
            .child(copy_from_name)
            .child(self.icon("chevron-down", 16., 16., self.color("foreground")));
        let create = self.text_button("profile-create", "Create", cx, |workbench, window, cx| workbench.create_new_profile(window, cx));
        let cancel = self.text_button("profile-cancel", "Cancel", cx, |workbench, _, cx| {
            let editor = &mut workbench.user_data_profiles.editor;
            editor.new_profile = None;
            editor.selected = workbench.user_data_profiles.state.as_ref().map(|state| state.current.id.clone());
            cx.notify();
        });
        div()
            .flex()
            .flex_col()
            .child(div().ml(px(27.)).h(px(40.)).flex().items_center().text_size(px(26.)).font_weight(FontWeight::SEMIBOLD).italic().child(editor.name.value.clone()))
            .child(
                div()
                    .mt(px(15.))
                    .ml(px(28.))
                    .child(self.profile_row_container("Name", div().flex().flex_col().gap(px(4.)).child(name).children(error)))
                    .child(self.profile_row_container("Copy from", div().flex().flex_col().gap(px(4.)).child(div().opacity(0.9).child("Select the profile source from which you want to copy contents")).child(copy_from)))
                    .child(div().mt(px(10.)).flex().gap(px(8.)).child(create).child(cancel)),
            )
    }

    /// The Copy from dropdown: None, then the existing profiles.
    fn show_copy_from_menu(&mut self, position: gpui::Point<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = &self.user_data_profiles.state else { return };
        let mut actions = Vec::new();
        let mut choices: Vec<Option<String>> = vec![None];
        choices.extend(state.profiles.profiles().into_iter().map(|profile| Some(profile.id)));
        let names: Vec<String> = std::iter::once("None".to_owned()).chain(state.profiles.profiles().into_iter().map(|profile| profile.name)).collect();
        for (index, (choice, name)) in choices.into_iter().zip(names).enumerate() {
            let id = format!("_profilesEditor.copyFrom.{index}");
            self.commands.register(id.clone(), move |workbench, _, _, cx| {
                if let Some(new_profile) = &mut workbench.user_data_profiles.editor.new_profile {
                    new_profile.copy_from = choice.clone();
                    new_profile.template = None;
                }
                cx.notify();
                Outcome::done()
            });
            actions.push(MenuAction::Command { id, title: name, enabled: true, checked: false, alt: None });
        }
        let anchor = gpui::Bounds::new(position, gpui::size(px(1.), px(1.)));
        let context = self.key_context(window);
        self.show_context_menu_actions(actions, &context, anchor, Vec::new(), window, cx);
    }

    /// "Add Folder": the picked folders open in the profile (`setProfileForWorkspace`).
    fn add_profile_folder(&mut self, profile: Profile, window: &mut Window, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: true, prompt: Some("Add Folder".into()) });
        cx.spawn_in(window, async move |workbench, cx| {
            let Ok(Ok(Some(paths))) = picked.await else { return };
            let _ = workbench.update(cx, |workbench, cx| {
                let Some(state) = workbench.user_data_profiles.state.as_mut() else { return };
                for path in paths {
                    let workspace = storage::profiles::WorkspaceId::Uri(crate::editor_history::file_uri_string(&path));
                    if let Err(error) = state.edit(|profiles, storage| profiles.set_profile_for_workspace(&workspace, &profile.id, storage)) {
                        eprintln!("[profiles] {error}");
                    }
                }
                workbench.profiles_changed(cx);
            });
        })
        .detach();
    }

    /// The Contents tree: the resources, Snippets and Extensions expandable to their files and extensions.
    fn profile_contents_tree(&self, profile: &Profile, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let expanded = &self.user_data_profiles.editor.expanded;
        let mut tree = div().mt(px(8.)).flex().flex_col().child(div().h(px(30.)).flex().items_center().pl(px(30.)).bg(self.color("keybindingTable.headerBackground")).font_weight(FontWeight::SEMIBOLD).child("Contents"));
        for (resource, title) in RESOURCES {
            let expandable = matches!(resource, "snippets" | "extensions");
            let twistie = expandable.then(|| self.icon(if expanded.contains(resource) { "chevron-down" } else { "chevron-right" }, 16., 16., self.color("foreground")));
            let row = div()
                .id(resource)
                .h(px(24.))
                .flex()
                .items_center()
                .pl(px(if expandable { 14. } else { 30. }))
                .when(expandable, |row| row.cursor_pointer())
                .on_click(cx.listener(move |workbench, _, _, cx| {
                    let expanded = &mut workbench.user_data_profiles.editor.expanded;
                    if !expanded.remove(resource) {
                        expanded.insert(resource);
                    }
                    cx.notify();
                }))
                .children(twistie)
                .child(title);
            tree = tree.child(row);
            if !expanded.contains(resource) {
                continue;
            }
            let children: Vec<String> = match resource {
                "snippets" => std::fs::read_dir(&profile.snippets_home).into_iter().flatten().flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect(),
                _ => self.profile_extensions(profile).iter().map(|extension| extension["displayName"].as_str().or(extension["identifier"]["id"].as_str()).unwrap_or_default().to_owned()).collect(),
            };
            for child in children {
                tree = tree.child(div().h(px(22.)).flex().items_center().pl(px(46.)).child(child));
            }
        }
        tree
    }

    fn profile_row_container(&self, label: &str, value: impl IntoElement) -> impl IntoElement {
        div().py(px(12.)).flex().flex_col().child(div().pb(px(5.)).font_weight(FontWeight::SEMIBOLD).child(label.to_owned())).child(value)
    }

    fn profile_checkbox(&self, id: &'static str, checked: bool, title: &str, cx: &mut Context<Self>, toggle: impl Fn(&mut Workbench, &mut Window, &mut Context<Workbench>) + 'static) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(9.))
            .cursor_pointer()
            .on_click(cx.listener(move |workbench, _, window, cx| toggle(workbench, window, cx)))
            .child(self.checkbox(checked, false))
            .child(title.to_owned())
    }

    fn text_button(&self, id: &'static str, label: &str, cx: &mut Context<Self>, run: impl Fn(&mut Workbench, &mut Window, &mut Context<Workbench>) + 'static) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .h(px(26.))
            .px(px(10.))
            .flex()
            .items_center()
            .rounded(px(4.))
            .bg(self.color("button.background"))
            .text_color(self.color("button.foreground"))
            .cursor_pointer()
            .hover(|button| button.bg(self.color("button.hoverBackground")))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |workbench, _, window, cx| run(workbench, window, cx)))
            .child(label.to_owned())
    }
}

/// `DEFAULT_ICON`: a profile without an icon shows the settings gear.
fn profile_icon(icon: Option<&str>) -> &str {
    icon.unwrap_or("settings-gear")
}
