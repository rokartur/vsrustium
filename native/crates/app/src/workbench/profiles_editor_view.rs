//! userDataProfilesEditor.ts: the Profiles editor in the modal editor part. The sidebar has the New Profile
//! button (Import Profile... in its dropdown) and the profiles; the contents view shows the selected profile
//! (Use for Current Window, Use for New Windows, its contents and the folders and workspaces using it), or the form
//! of a new profile (name, the profile to copy from, Create).

use std::cell::Cell;
use std::collections::{BTreeSet, HashSet};
use std::rc::Rc;

use gpui::{AnyElement, Context, FocusHandle, FontWeight, MouseButton, PathPromptOptions, Window, div, prelude::*, px};
use serde_json::{Value, json};
use storage::profiles::{Options, Profile};

use super::Workbench;
use super::icon_select_box::{IconSelectBox, IconTarget};
use super::input_box_view::{InputId, InputLook, InputView};
use super::keybindings_editor_view::ModalHeader;
use crate::commands::Outcome;
use crate::input_box::InputBox;
use crate::main_thread_quick_open::Owner;
use crate::menus::MenuAction;
use crate::profile_template::{self, Flags, PROFILE_EXTENSION};
use crate::quick_input::{Entry, Event, Item, Kind, QuickInput};

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
    pub icon_picker: Option<IconSelectBox>,
    pub icon_search_focus: Option<FocusHandle>,
    pub icon_grid_focus: Option<FocusHandle>,
    /// The Icon row's icon, the picker's anchor.
    icon_bounds: Rc<Cell<gpui::Bounds<gpui::Pixels>>>,
}

/// The form of a new profile: copied from a profile, or from an imported template.
pub struct NewProfile {
    pub copy_from: Option<String>,
    pub template: Option<Value>,
    pub error: Option<String>,
    /// The name the form gave, replaced when the source changes while the user kept it.
    default_name: String,
    pub icon: Option<String>,
    /// The source's icon, replaced as `default_name` is.
    default_icon: Option<String>,
    /// `previewProfile`: the profile Preview created, which Create keeps and Cancel removes.
    preview: Option<Profile>,
    /// `flags`: the resources that use the Default profile's.
    use_default: BTreeSet<&'static str>,
    /// `copyFlags`: the resources copied from the source.
    copy: BTreeSet<&'static str>,
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
        // The New Profile dropdown's "Import Profile..." action; upstream runs it without a command.
        self.commands.register("_profilesEditor.importProfile", |workbench, _, window, cx| {
            workbench.open_profiles_editor(window, cx);
            workbench.import_profile(window, cx);
            Outcome::done()
        });
        self.register_icon_select_box_commands();
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
        editor.icon_picker = None;
        cx.notify();
    }

    /// `createNewProfile(copyFrom)`: the new profile form, named after its source or "Untitled N", with its icon.
    fn start_new_profile(&mut self, copy_from: Option<String>, template: Option<Value>, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.new_profile_default_name(copy_from.as_deref(), template.as_ref());
        let icon = self.new_profile_default_icon(copy_from.as_deref(), template.as_ref());
        let copy = copy_flags(copy_from.as_deref(), template.as_ref());
        let editor = &mut self.user_data_profiles.editor;
        editor.icon_picker = None;
        editor.new_profile = Some(NewProfile { copy_from, template, error: None, default_name: name.clone(), icon: icon.clone(), default_icon: icon, preview: None, use_default: BTreeSet::new(), copy });
        editor.selected = None;
        editor.name = InputBox::new(name);
        editor.name_view.reset();
        // `render`'s `profileTree.focusFirst()`: the form's tree takes the focus, not the Name input.
        if let Some(focus) = editor.focus.clone() {
            window.focus(&focus, cx);
        }
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
        editor.icon_picker = None;
        editor.name = InputBox::new(profile.name.clone());
        editor.name_view.reset();
        cx.notify();
    }

    /// `saveNewProfile(transient)`: Create keeps a previewed profile, else creates the profile (replacing the one of the
    /// same name when the form copies from a source and the user confirms) and selects it; Preview creates it and opens
    /// it in a new window, keeping the form.
    fn save_new_profile(&mut self, preview: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(new_profile) = &self.user_data_profiles.editor.new_profile else { return };
        if let Some(previewed) = new_profile.preview.clone() {
            if !preview {
                self.finish_new_profile(&previewed, cx);
            }
            return;
        }
        if let Some(error) = self.new_profile_error() {
            return self.set_new_profile_error(&error, cx);
        }
        let name = self.user_data_profiles.editor.name.value.trim().to_owned();
        let profiles = self.user_data_profiles.state.as_ref().map(|state| state.profiles.profiles()).unwrap_or_default();
        let existing = profiles.into_iter().find(|profile| profile.name == name && !profile.is_transient);
        let Some(existing) = existing.filter(|_| !preview) else { return self.create_profile_from_form(None, preview, cx) };
        // `getProfileToImport`: replacing asks first.
        let message = format!("Profile with name '{name}' already exists. Do you want to replace its contents?");
        cx.spawn_in(window, async move |workbench, cx| {
            if !super::explorer_actions::confirm(&workbench, cx, gpui::PromptLevel::Info, message, None, "Replace").await {
                return;
            }
            let _ = workbench.update(cx, |workbench, cx| workbench.create_profile_from_form(Some(existing), false, cx));
        })
        .detach();
    }

    /// `NewProfileElement.validate`.
    fn new_profile_error(&self) -> Option<String> {
        let editor = &self.user_data_profiles.editor;
        let new_profile = editor.new_profile.as_ref()?;
        let name = editor.name.value.trim();
        if name.is_empty() {
            return Some("Profile name is required and must be a non-empty value.".into());
        }
        let has_source = new_profile.copy_from.is_some() || new_profile.template.is_some();
        let profiles = self.user_data_profiles.state.as_ref().map(|state| state.profiles.profiles()).unwrap_or_default();
        if !has_source && profiles.iter().any(|profile| profile.name == name) {
            return Some(format!("Profile with name {name} already exists."));
        }
        if ["settings", "keybindings", "tasks", "snippets", "extensions"].iter().all(|resource| new_profile.use_default.contains(resource)) {
            return Some("The profile should contain at least one configuration.".into());
        }
        None
    }

    /// `createFromProfile`, `createProfileFromTemplate` or `createProfile` with the form's name, icon and flags; a
    /// preview of a taken name gets the next free `<name> <n>`.
    fn create_profile_from_form(&mut self, existing: Option<Profile>, preview: bool, cx: &mut Context<Self>) {
        let Some(new_profile) = &self.user_data_profiles.editor.new_profile else { return };
        let Some(state) = self.user_data_profiles.state.as_ref() else { return };
        let profiles = state.profiles.profiles();
        let mut name = self.user_data_profiles.editor.name.value.trim().to_owned();
        if preview && profiles.iter().any(|profile| profile.name == name) {
            name = format!("{name} {}", name_index(profiles.iter().map(|profile| profile.name.as_str()), &name));
        }
        let copy_from = new_profile.copy_from.clone().and_then(|id| profiles.into_iter().find(|profile| profile.id == id));
        let icon = new_profile.icon.clone();
        let template = match (&new_profile.template, &copy_from) {
            (Some(template), _) => Some(template.clone()),
            (None, Some(from)) => Some(self.profile_template(from, &name)),
            (None, None) => None,
        };
        let template = template.map(|template| copied_template(template, &new_profile.copy, &new_profile.use_default));
        let use_default_flags: Option<storage::profiles::UseDefaultFlags> = (!new_profile.use_default.is_empty()).then(|| new_profile.use_default.iter().map(|resource| (resource.to_string(), true)).collect());
        let state = self.user_data_profiles.state.as_mut().expect("checked above");
        let saved = match existing {
            Some(existing) if existing.is_default => Ok(existing),
            Some(existing) => {
                let update = storage::profiles::UpdateOptions { icon: Some(icon), use_default_flags: Some(use_default_flags.unwrap_or_default()), ..Default::default() };
                state.edit(|profiles, storage| profiles.update_profile(&existing.id, update, storage)).map(|(profile, _)| profile)
            }
            None => state.edit(|profiles, storage| profiles.create_named_profile(&name, Options { icon, use_default_flags, ..Options::default() }, None, storage)).map(|(profile, _)| profile),
        };
        let profile = match saved {
            Ok(profile) => profile,
            Err(error) => return self.set_new_profile_error(&error, cx),
        };
        if let Some(template) = template
            && let Err(error) = self.apply_profile_template(&template, &profile, cx)
        {
            self.notify_error(error);
        }
        if !preview {
            return self.finish_new_profile(&profile, cx);
        }
        // shortcut: the preview profile is stored, not transient: each window is its own process, so a transient
        // profile would not reach the preview window; Cancel removes it.
        if let Some(new_profile) = &mut self.user_data_profiles.editor.new_profile {
            new_profile.preview = Some(profile.clone());
        }
        self.profiles_changed(cx);
        drop(self.open_window_with_profile_flags(vec!["--profile".into(), profile.name.clone()]));
        cx.notify();
    }

    /// `removeNewProfile` after a save: the created profile is selected.
    fn finish_new_profile(&mut self, profile: &Profile, cx: &mut Context<Self>) {
        let editor = &mut self.user_data_profiles.editor;
        editor.new_profile = None;
        editor.icon_picker = None;
        editor.selected = Some(profile.id.clone());
        editor.name = InputBox::new(profile.name.clone());
        editor.name_view.reset();
        self.profiles_changed(cx);
        cx.notify();
    }

    /// `discardNewProfile`: removes the previewed profile, else closes the form.
    fn discard_new_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(new_profile) = &mut self.user_data_profiles.editor.new_profile else { return };
        if let Some(previewed) = new_profile.preview.take() {
            return self.remove_profile(&previewed, window, cx);
        }
        let editor = &mut self.user_data_profiles.editor;
        editor.new_profile = None;
        editor.icon_picker = None;
        editor.selected = self.user_data_profiles.state.as_ref().map(|state| state.current.id.clone());
        cx.notify();
    }

    /// The new profile's name for its source: the template's, "<profile> (Copy)", or "Untitled".
    fn new_profile_default_name(&self, copy_from: Option<&str>, template: Option<&Value>) -> String {
        if let Some(name) = template.and_then(|template| template["name"].as_str()) {
            return name.to_owned();
        }
        let profiles = self.user_data_profiles.state.as_ref().map(|state| state.profiles.profiles()).unwrap_or_default();
        match copy_from.and_then(|id| profiles.iter().find(|profile| profile.id == id)) {
            Some(profile) => format!("{} (Copy)", profile.name),
            None => untitled_name(&profiles),
        }
    }

    /// The new profile's icon for its source: the template's or the copied profile's.
    fn new_profile_default_icon(&self, copy_from: Option<&str>, template: Option<&Value>) -> Option<String> {
        if let Some(template) = template {
            return template["icon"].as_str().map(str::to_owned);
        }
        let profiles = self.user_data_profiles.state.as_ref().map(|state| state.profiles.profiles()).unwrap_or_default();
        copy_from.and_then(|id| profiles.into_iter().find(|profile| profile.id == id)).and_then(|profile| profile.icon)
    }

    /// `root.icon = selectedIcon.id`: the new profile form keeps it, a profile saves it (`updateProfile`).
    pub(super) fn set_profile_icon(&mut self, target: &IconTarget, icon: &str, cx: &mut Context<Self>) {
        let id = match target {
            IconTarget::NewProfile => {
                if let Some(new_profile) = &mut self.user_data_profiles.editor.new_profile {
                    new_profile.icon = Some(icon.to_owned());
                }
                return cx.notify();
            }
            IconTarget::Profile(id) => id,
        };
        let Some(state) = self.user_data_profiles.state.as_mut() else { return };
        let update = storage::profiles::UpdateOptions { icon: Some(Some(icon.to_owned())), ..Default::default() };
        match state.edit(|profiles, storage| profiles.update_profile(id, update, storage)) {
            Ok((updated, _)) => {
                if state.current.id == updated.id {
                    state.current = updated;
                }
                self.profiles_changed(cx);
            }
            Err(error) => self.notify_error(error),
        }
        cx.notify();
    }

    /// `NewProfileResourceTreeRenderer`'s radio: [copy,] Default, None.
    fn set_new_profile_source(&mut self, resource: &'static str, index: usize, cx: &mut Context<Self>) {
        let Some(new_profile) = &mut self.user_data_profiles.editor.new_profile else { return };
        let has_source = new_profile.copy_from.is_some() || new_profile.template.is_some();
        let (use_default, copy) = match has_source {
            true => (index == 1, index == 0),
            false => (index == 0, false),
        };
        set_member(&mut new_profile.use_default, resource, use_default);
        set_member(&mut new_profile.copy, resource, copy);
        cx.notify();
    }

    /// `ExistingProfileResourceTreeRenderer`'s radio: `setFlag`, saved to the profile's `useDefaultFlags`; the
    /// current profile's files move with it.
    fn set_profile_use_default(&mut self, profile_id: &str, resource: &'static str, use_default: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.user_data_profiles.state.as_mut() else { return };
        let Some(profile) = state.profiles.profiles().into_iter().find(|profile| profile.id == profile_id) else { return };
        let mut flags: storage::profiles::UseDefaultFlags = profile.use_default_flags.clone().unwrap_or_default();
        flags.insert(resource.to_owned(), use_default);
        flags.retain(|_, used| *used);
        let update = storage::profiles::UpdateOptions { use_default_flags: Some(flags), ..Default::default() };
        let updated = match state.edit(|profiles, storage| profiles.update_profile(profile_id, update, storage)) {
            Ok((updated, _)) => updated,
            Err(error) => return self.notify_error(error),
        };
        let is_current = state.current.id == updated.id;
        self.profiles_changed(cx);
        if is_current {
            self.change_current_profile(updated, window, cx);
        }
    }

    /// The Contents row of the new profile form: the description, then each resource with its Source radio.
    fn new_profile_source_contents(&self, bullets: Vec<(String, String)>, rows: Vec<(&'static str, &'static str, Vec<String>, usize)>, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let mut tree = div().flex().flex_col().child(self.source_description(bullets)).child(self.contents_header());
        for (resource, title, options, active) in rows {
            let radio = self.radio(resource, &options, active, cx, move |workbench, index, _, cx| workbench.set_new_profile_source(resource, index, cx));
            tree = tree.child(div().h(px(24.)).flex().items_center().child(div().flex_1().pl(px(30.)).child(title)).child(div().flex_1().child(radio)).child(div().w(px(80.))));
        }
        tree
    }

    /// `updateDescription` of a profile that is not the default one.
    fn source_description(&self, bullets: Vec<(String, String)>) -> impl IntoElement + use<> {
        let mut description = div().flex().flex_col().line_height(px(18.)).opacity(0.9).child("Configure source of contents for this profile");
        for (name, text) in bullets {
            description = description.child(div().pl(px(17.)).flex().child(div().w(px(13.)).child("\u{2022}")).child(div().italic().child(format!("{name}:"))).child(div().ml(px(4.)).child(text)));
        }
        description
    }

    /// `.profile-content-tree-header` of a profile that is not the default one: Contents and Source.
    fn contents_header(&self) -> impl IntoElement + use<> {
        div()
            .mt(px(5.))
            .mb(px(2.))
            .h(px(30.))
            .flex()
            .items_center()
            .bg(self.color("keybindingTable.headerBackground"))
            .font_weight(FontWeight::SEMIBOLD)
            .child(div().flex_1().pl(px(30.)).child("Contents"))
            .child(div().flex_1().child("Source"))
            .child(div().w(px(80.)))
    }

    /// radio.ts `Radio`: a row of buttons, the active one in the radio colors.
    fn radio(&self, id: &'static str, options: &[String], active: usize, cx: &mut Context<Self>, select: impl Fn(&mut Workbench, usize, &mut Window, &mut Context<Workbench>) + 'static) -> gpui::Div {
        let select = Rc::new(select);
        let last = options.len().saturating_sub(1);
        let mut radio = div().flex();
        for (index, option) in options.iter().enumerate() {
            let is_active = index == active;
            let (foreground, background, border) = match is_active {
                true => (self.color("radio.activeForeground"), self.color("radio.activeBackground"), self.color("radio.activeBorder")),
                false => (self.color("foreground"), gpui::transparent_black().into(), self.color("radio.inactiveBorder")),
            };
            let select = select.clone();
            let button = div()
                .id((id, index))
                .h(px(22.))
                .px(px(6.))
                .flex()
                .items_center()
                .text_size(px(11.7))
                .text_color(foreground)
                .bg(background)
                .border_color(border)
                .border_t_1()
                .border_b_1()
                // `:not(.active):not(:last-child)` has no right border, `.previous-active` no left one.
                .when(is_active || index == last, |button| button.border_r_1())
                .when(index == 0 || index != active + 1, |button| button.border_l_1())
                .when(index == 0, |button| button.rounded_l(px(3.)))
                .when(index == last, |button| button.rounded_r(px(3.)))
                .cursor_pointer()
                .on_click(cx.listener(move |workbench, _, window, cx| select(workbench, index, window, cx)))
                .child(option.clone());
            radio = radio.child(button);
        }
        radio
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
        self.user_data_profiles.pending_installs.insert(profile.id.clone(), to_install);
        cx.notify();
        Ok(())
    }

    /// `exportProfile`: the template saved to a `.code-profile` file (FileUserDataProfileContentHandler).
    fn export_profile(&mut self, profile: Profile, window: &mut Window, cx: &mut Context<Self>) {
        let template = self.profile_template(&profile, &profile.name);
        self.save_profile_template(template, profile.name, window, cx);
    }

    /// `exportNewProfile`: the copied profile's contents under the form's name, icon and flags.
    fn export_new_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(new_profile) = &self.user_data_profiles.editor.new_profile else { return };
        let Some(state) = &self.user_data_profiles.state else { return };
        let Some(from) = new_profile.copy_from.as_ref().and_then(|id| state.profiles.profiles().into_iter().find(|profile| profile.id == *id)) else { return };
        let name = self.user_data_profiles.editor.name.value.trim().to_owned();
        let profile = Profile { name: name.clone(), icon: new_profile.icon.clone(), ..from };
        let template = copied_template(self.profile_template(&profile, &name), &new_profile.copy, &new_profile.use_default);
        self.save_profile_template(template, name, window, cx);
    }

    /// FileUserDataProfileContentHandler: the template saved to a picked `.code-profile` file.
    fn save_profile_template(&mut self, template: Value, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let file_name = format!("{}.{PROFILE_EXTENSION}", name.replace('/', "-"));
        let picked = cx.prompt_for_new_path(&self.default_file_path(), Some(&file_name));
        cx.spawn_in(window, async move |workbench, cx| {
            let Ok(Ok(Some(path))) = picked.await else { return };
            let written = std::fs::write(&path, template.to_string());
            let _ = workbench.update(cx, |workbench, cx| {
                match written {
                    Ok(()) => drop(workbench.prompt_notification(crate::notifications::Severity::Info, format!("Profile '{name}' was exported successfully."), &[])),
                    Err(error) => workbench.notify_error(format!("{}: {error}", path.display())),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// `importProfile`: a quick pick of the typed template URL ("Import from URL") and Select File...
    fn import_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut input = QuickInput::new(0, Kind::QuickPick);
        input.title = Some("Import from Profile Template...".into());
        input.placeholder = Some("Provide Profile Template URL".into());
        input.ignore_focus_out = true;
        input.match_on_label = false;
        input.match_on_description = false;
        input.set_items(import_items(""));
        let id = self.quick_inputs.add(input, Owner::ProfileTemplateImport);
        self.show_quick_input(id, window, cx);
        self.flush_quick_input_events(window, cx);
    }

    pub(crate) fn profile_template_import_event(&mut self, id: i64, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            Event::ChangeValue(value) => {
                if let Some(input) = self.quick_inputs.get_mut(id) {
                    input.set_items(import_items(&value));
                }
                cx.notify();
            }
            Event::Accept(_) => {
                let Some(input) = self.quick_inputs.get(id) else { return };
                let url = input.active_handles().first().is_some_and(|&handle| handle == IMPORT_URL_HANDLE).then(|| input.input_box.value.trim().to_owned());
                self.hide_quick_input_by_id(id, window, cx);
                match url {
                    Some(url) => self.import_profile_from_url(url, window, cx),
                    None => self.import_profile_from_file(window, cx),
                }
            }
            Event::Hide => self.quick_inputs.remove(id),
            _ => {}
        }
    }

    /// `resolveProfileContent` of an http(s) URL: the template the request service fetches.
    fn import_profile_from_url(&mut self, url: String, window: &mut Window, cx: &mut Context<Self>) {
        let fetched = cx.background_executor().spawn(async move {
            let response = exthost::request::request(&exthost::request::Request::get(&url)).map_err(|error| error.to_string())?;
            let content = response.as_text().map_err(|error| error.to_string())?.unwrap_or_default();
            profile_template::parse_template(&content)
        });
        cx.spawn_in(window, async move |workbench, cx| {
            let template = fetched.await;
            let _ = workbench.update_in(cx, |workbench, window, cx| match template {
                Ok(template) => workbench.start_new_profile(None, Some(template), window, cx),
                Err(error) => workbench.notify_error(error),
            });
        })
        .detach();
    }

    /// `getProfileUriFromFileSystem`: "Select Profile Template File", then the new profile form with its contents.
    fn import_profile_from_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    /// The `...` actions of a profile row (`createProfileElement`'s secondary actions): Duplicate..., Export..., then
    /// Delete for a profile that is not the default one.
    fn show_profile_actions(&mut self, profile: Profile, anchor: gpui::Bounds<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let mut actions = Vec::new();
        let duplicate = profile.id.clone();
        self.register_row_action(&mut actions, "duplicate", "Duplicate...", move |workbench, window, cx| workbench.start_new_profile(Some(duplicate.clone()), None, window, cx));
        let export = profile.clone();
        self.register_row_action(&mut actions, "export", "Export...", move |workbench, window, cx| workbench.export_profile(export.clone(), window, cx));
        if !profile.is_default {
            actions.push(MenuAction::Separator);
            self.register_row_action(&mut actions, "delete", "Delete", move |workbench, window, cx| workbench.confirm_remove_profile(profile.clone(), window, cx));
        }
        let context = self.key_context(window);
        self.show_context_menu_actions(actions, &context, anchor, Vec::new(), window, cx);
    }

    /// The new profile row's `...` action: Export..., while the form copies from a profile.
    fn show_new_profile_actions(&mut self, anchor: gpui::Bounds<gpui::Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let mut actions = Vec::new();
        self.register_row_action(&mut actions, "exportNew", "Export...", |workbench, window, cx| workbench.export_new_profile(window, cx));
        let context = self.key_context(window);
        self.show_context_menu_actions(actions, &context, anchor, Vec::new(), window, cx);
    }

    fn register_row_action(&mut self, actions: &mut Vec<MenuAction>, id: &str, title: &str, run: impl Fn(&mut Workbench, &mut Window, &mut Context<Workbench>) + 'static) {
        let id = format!("_profilesEditor.{id}");
        self.commands.register(id.clone(), move |workbench, _, window, cx| {
            run(workbench, window, cx);
            Outcome::done()
        });
        actions.push(MenuAction::Command { id, title: title.into(), enabled: true, checked: false, alt: None });
    }

    /// `removeProfile`: deletes after the confirmation.
    fn confirm_remove_profile(&mut self, profile: Profile, window: &mut Window, cx: &mut Context<Self>) {
        let message = format!("Are you sure you want to delete the profile '{}'?", profile.name);
        cx.spawn_in(window, async move |workbench, cx| {
            if super::explorer_actions::confirm(&workbench, cx, gpui::PromptLevel::Info, message, None, "Delete").await {
                let _ = workbench.update_in(cx, |workbench, window, cx| workbench.remove_profile(&profile, window, cx));
            }
        })
        .detach();
    }

    pub(super) fn profiles_editor_widget(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.user_data_profiles.editor.open {
            return None;
        }
        let header = ModalHeader { icon: "settings", title: "Profiles", json_title: "", json_command: "" };
        let focus = self.user_data_profiles.editor.focus.clone()?;
        let modal = self.modal_editor_part(header, &focus, window, cx, |workbench, width, height, window, cx| workbench.profiles_editor_pane(width, height, window, cx));
        let picker = self.icon_select_box_widget(self.user_data_profiles.editor.icon_bounds.get(), window, cx);
        Some(div().absolute().inset_0().child(modal).children(picker).into_any_element())
    }

    fn profiles_editor_pane(&mut self, width: f32, _height: f32, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(state) = &self.user_data_profiles.state else { return div().into_any_element() };
        let previewed = self.user_data_profiles.editor.new_profile.as_ref().and_then(|new_profile| new_profile.preview.as_ref()).map(|profile| profile.id.clone());
        let profiles: Vec<Profile> = state.profiles.profiles().into_iter().filter(|profile| Some(&profile.id) != previewed.as_ref()).collect();
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
            .child(div().flex_1().min_w_0().h_full().flex().flex_col().child(contents))
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
                        let id = "_profilesEditor.importProfile".to_owned();
                        let actions = vec![MenuAction::Command { id, title: "Import Profile...".into(), enabled: true, checked: false, alt: None }];
                        let position = event.position();
                        let anchor = gpui::Bounds::new(position, gpui::size(px(1.), px(1.)));
                        let context = workbench.key_context(window);
                        workbench.show_context_menu_actions(actions, &context, anchor, Vec::new(), window, cx);
                    })),
            );
        let selected = self.user_data_profiles.editor.selected.clone();
        let mut rows: Vec<AnyElement> = Vec::new();
        for profile in profiles {
            let is_selected = selected.as_deref() == Some(profile.id.as_str());
            rows.push(self.profile_row(profile, profile.id == current.id, is_selected, cx).into_any_element());
        }
        rows.extend(self.new_profile_row(profiles, cx).map(IntoElement::into_any_element));
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

    /// `ProfileElementRenderer` of the new profile: its icon, the italic name, the dirty dot, and its actions, Cancel
    /// (the trash icon) and, while it copies from a profile, Export... in the `...` menu.
    fn new_profile_row(&self, profiles: &[Profile], cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let editor = &self.user_data_profiles.editor;
        let new_profile = editor.new_profile.as_ref()?;
        // The focus is in the form, so the sidebar list shows its selection inactive.
        let foreground = self.foreground_or("list.inactiveSelectionForeground");
        let exportable = new_profile.copy_from.as_ref().is_some_and(|id| profiles.iter().any(|profile| profile.id == *id));
        let actions = div()
            .flex_1()
            .flex()
            .justify_end()
            .mr(px(10.))
            .child(
                self.icon("trash", 22., 16., foreground)
                    .id("new-profile-cancel")
                    .cursor_pointer()
                    .child(self.hover_target("new-profile-cancel-hover", super::hover::HoverContent::Text("Cancel".into()), super::hover::HoverOptions::instant()))
                    .on_click(cx.listener(|workbench, _, window, cx| workbench.discard_new_profile(window, cx))),
            )
            .when(exportable, |actions| {
                actions.child(self.icon("ellipsis", 22., 16., foreground).id("new-profile-actions").cursor_pointer().on_click(cx.listener(|workbench, event: &gpui::ClickEvent, window, cx| {
                    let anchor = gpui::Bounds::new(event.position(), gpui::size(px(1.), px(1.)));
                    workbench.show_new_profile_actions(anchor, window, cx);
                })))
            });
        let row = div()
            .id("new-profile-row")
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .pl(px(20.))
            .cursor_pointer()
            .text_color(foreground)
            .bg(self.color("list.inactiveSelectionBackground"))
            .child(self.icon(super::icon_select_box::profile_icon_file(new_profile.icon.as_deref()), 22., 16., foreground))
            .child(div().ml(px(5.)).italic().truncate().child(editor.name.value.clone()))
            .child(self.icon("circle-filled", 16., 16., foreground).ml(px(5.)))
            .child(actions);
        Some(row)
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
            .child(self.icon(super::icon_select_box::profile_icon_file(profile.icon.as_deref()), 22., 16., foreground))
            .child(div().ml(px(5.)).truncate().child(profile.name.clone()))
            .children(description)
            // `.profile-tree-item-actions-container` shows on the selected, focused or hovered row only.
            .group("profile-row")
            .child(actions.when(!selected, |actions| actions.invisible().group_hover("profile-row", |actions| actions.visible())))
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
        let description = match profile.is_default {
            true => div().line_height(px(18.)).opacity(0.9).child("Browse contents of this profile").into_any_element(),
            false => self.source_description(Vec::new()).into_any_element(),
        };
        let contents = self.profile_row_container("Contents", div().flex().flex_col().child(description).child(self.profile_contents_tree(profile, cx)));
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
        let icon = (!profile.is_default).then(|| self.profile_row_container("Icon", self.profile_icon_row(profile.icon.as_deref(), IconTarget::Profile(profile.id.clone()), self.user_data_profiles.editor.icon_bounds.clone(), cx)));
        let body = div().id("profile-body").flex_1().min_h_0().overflow_y_scroll().child(div().mt(px(15.)).ml(px(28.)).children(name).children(icon).children(use_for_current).child(use_for_new).child(contents).child(folders));
        div().size_full().flex().flex_col().child(title).child(body)
    }

    fn new_profile_contents(&self, profiles: &[Profile], window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let editor = &self.user_data_profiles.editor;
        let new_profile = editor.new_profile.as_ref().expect("the new profile form is shown");
        let name = self.profile_name_input(window, cx);
        let error = new_profile.error.clone().or_else(|| self.new_profile_error()).map(|error| div().w(px(400.)).px(px(6.)).py(px(4.)).border_1().border_color(self.color("inputValidation.errorBorder")).bg(self.color("inputValidation.errorBackground")).child(error));
        let copy_from_name = match (&new_profile.template, &new_profile.copy_from) {
            (Some(template), _) => template["name"].as_str().unwrap_or_default().to_owned(),
            (None, Some(id)) => profiles.iter().find(|profile| profile.id == *id).map(|profile| profile.name.clone()).unwrap_or_default(),
            (None, None) => "None".to_owned(),
        };
        let copy_from = div()
            .id("profile-copy-from")
            // `.profile-select-container`: 250px wide, 5px below the description; the select box's 18px line, 8px left padding and 2px corners.
            .mt(px(5.))
            .w(px(250.))
            .h(px(20.))
            .flex()
            .items_center()
            .justify_between()
            .pl(px(8.))
            .pr(px(4.))
            .rounded(px(2.))
            .border_1()
            .border_color(self.color("dropdown.border"))
            .bg(self.color("dropdown.background"))
            .cursor_pointer()
            .on_click(cx.listener(|workbench, event: &gpui::ClickEvent, window, cx| workbench.show_copy_from_menu(event.position(), window, cx)))
            .child(copy_from_name.clone())
            .child(self.icon("chevron-down", 16., 16., self.color("foreground")));
        // `getCopyFromName`: the Default profile shows as "Default (Copy)".
        let source = (new_profile.copy_from.is_some() || new_profile.template.is_some()).then(|| {
            let is_default = new_profile.copy_from.as_ref().is_some_and(|id| profiles.iter().any(|profile| profile.id == *id && profile.is_default));
            (if is_default { format!("{copy_from_name} (Copy)") } else { copy_from_name.clone() }, copy_from_name.clone())
        });
        let mut bullets = Vec::new();
        if let Some((option, name)) = &source {
            bullets.push((option.clone(), format!("Copy contents from the {name} profile")));
        }
        bullets.push(("Default".to_owned(), "Use contents from the Default profile".to_owned()));
        bullets.push(("None".to_owned(), "Create empty contents".to_owned()));
        let rows = RESOURCES
            .iter()
            .map(|&(resource, title)| {
                let use_default = new_profile.use_default.contains(resource);
                match &source {
                    Some((option, _)) => (resource, title, vec![option.clone(), "Default".to_owned(), "None".to_owned()], if new_profile.copy.contains(resource) { 0 } else if use_default { 1 } else { 2 }),
                    None => (resource, title, vec!["Default".to_owned(), "None".to_owned()], if use_default { 0 } else { 1 }),
                }
            })
            .collect();
        let valid = self.new_profile_error().is_none();
        let replaces = new_profile.copy_from.is_some() && profiles.iter().any(|profile| !profile.is_transient && profile.name == editor.name.value.trim());
        let cancel = self.title_button("profile-cancel", "Cancel", false, true, cx, |workbench, window, cx| workbench.discard_new_profile(window, cx));
        let preview = self.title_button("profile-preview", "Preview", false, valid && new_profile.preview.is_none(), cx, |workbench, window, cx| workbench.save_new_profile(true, window, cx));
        let create = self.title_button("profile-create", if replaces { "Replace" } else { "Create" }, true, valid, cx, |workbench, window, cx| workbench.save_new_profile(false, window, cx));
        let icon = self.profile_icon_row(new_profile.icon.as_deref(), IconTarget::NewProfile, editor.icon_bounds.clone(), cx);
        let body = div()
            .id("new-profile-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(
                div()
                    .mt(px(15.))
                    .ml(px(28.))
                    .child(self.profile_row_container("Name", div().flex().flex_col().gap(px(4.)).child(name).children(error)))
                    .child(self.profile_row_container("Icon", icon))
                    .child(self.profile_row_container("Copy from", div().flex().flex_col().child(div().opacity(0.9).child("Select the profile source from which you want to copy contents")).child(copy_from)))
                    .child(self.profile_row_container("Contents", self.new_profile_source_contents(bullets, rows, cx))),
            );
        // `.profile-button-container`: the secondary buttons, then the primary one, under the scrolling tree.
        let buttons = div().flex_none().mt(px(10.)).ml(px(30.)).py(px(12.)).flex().items_center().child(cancel).child(preview).child(create);
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(div().ml(px(27.)).h(px(40.)).flex_none().flex().items_center().text_size(px(26.)).font_weight(FontWeight::SEMIBOLD).child(editor.name.value.clone()))
            .child(body)
            .child(buttons)
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
                let name = workbench.new_profile_default_name(choice.as_deref(), None);
                let icon = workbench.new_profile_default_icon(choice.as_deref(), None);
                let editor = &mut workbench.user_data_profiles.editor;
                if let Some(new_profile) = &mut editor.new_profile {
                    if new_profile.icon == new_profile.default_icon {
                        new_profile.icon = icon.clone();
                    }
                    new_profile.default_icon = icon.clone();
                    new_profile.copy_from = choice.clone();
                    new_profile.template = None;
                    new_profile.copy = copy_flags(choice.as_deref(), None);
                    if editor.name.value == new_profile.default_name {
                        editor.name = InputBox::new(name.clone());
                    }
                    new_profile.default_name = name.clone();
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

    /// The Contents tree: the resources, Snippets and Extensions expandable to their files and extensions; a profile
    /// that is not the default one has a Source radio per resource (`ExistingProfileResourceTreeRenderer`).
    fn profile_contents_tree(&self, profile: &Profile, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let expanded = &self.user_data_profiles.editor.expanded;
        let mut tree = match profile.is_default {
            true => div().mt(px(8.)).flex().flex_col().child(div().h(px(30.)).flex().items_center().pl(px(30.)).bg(self.color("keybindingTable.headerBackground")).font_weight(FontWeight::SEMIBOLD).child("Contents")),
            false => div().flex().flex_col().child(self.contents_header()),
        };
        for (resource, title) in RESOURCES {
            let expandable = matches!(resource, "snippets" | "extensions");
            let twistie = expandable.then(|| self.icon(if expanded.contains(resource) { "chevron-down" } else { "chevron-right" }, 16., 16., self.color("foreground")));
            let label = div()
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
            let radio = (!profile.is_default).then(|| {
                let use_default = profile.use_default_flags.as_ref().is_some_and(|flags| flags.get(resource) == Some(&true));
                let options = vec!["Default".to_owned(), profile.name.clone()];
                let id = profile.id.clone();
                self.radio(resource, &options, if use_default { 0 } else { 1 }, cx, move |workbench, index, window, cx| workbench.set_profile_use_default(&id, resource, index == 0, window, cx))
            });
            let row = match radio {
                Some(radio) => div().flex().items_center().child(label.flex_1()).child(div().flex_1().child(radio)).child(div().w(px(80.))).into_any_element(),
                None => label.into_any_element(),
            };
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

    /// A `.profile-button-container` button: `padding: 4px 10px; margin-right: 10px`, secondary or primary, dimmed and
    /// inert while disabled.
    fn title_button(&self, id: &'static str, label: &str, primary: bool, enabled: bool, cx: &mut Context<Self>, run: impl Fn(&mut Workbench, &mut Window, &mut Context<Workbench>) + 'static) -> gpui::Stateful<gpui::Div> {
        let (background, foreground, hover) = match primary {
            true => ("button.background", "button.foreground", "button.hoverBackground"),
            false => ("button.secondaryBackground", "button.secondaryForeground", "button.secondaryHoverBackground"),
        };
        let hover = self.color(hover);
        div()
            .id(id)
            .mr(px(10.))
            .px(px(10.))
            .py(px(4.))
            .rounded(px(4.))
            .bg(self.color(background))
            .text_color(self.color(foreground))
            .when_some(self.theme.color(if primary { "button.border" } else { "button.secondaryBorder" }), |button, border| button.border_1().border_color(border))
            .when(!enabled, |button| button.opacity(0.4))
            .when(enabled, |button| {
                button
                    .cursor_pointer()
                    .hover(move |button| button.bg(hover))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |workbench, _, window, cx| run(workbench, window, cx)))
            })
            .child(label.to_owned())
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

/// `getCopyFlagsFrom` and `initialize`: every resource of a profile, the resources a template has.
fn copy_flags(copy_from: Option<&str>, template: Option<&Value>) -> BTreeSet<&'static str> {
    match (template, copy_from) {
        (Some(template), _) => RESOURCES.iter().map(|(resource, _)| *resource).filter(|resource| !template[*resource].is_null()).collect(),
        (None, Some(_)) => RESOURCES.iter().map(|(resource, _)| *resource).collect(),
        (None, None) => BTreeSet::new(),
    }
}

/// `resourceTypeFlags: copyFlags`: the template keeps the resources copied and not taken from the Default profile.
fn copied_template(mut template: Value, copy: &BTreeSet<&'static str>, use_default: &BTreeSet<&'static str>) -> Value {
    if let Some(resources) = template.as_object_mut() {
        for (resource, _) in RESOURCES {
            if !copy.contains(resource) || use_default.contains(resource) {
                resources.remove(resource);
            }
        }
    }
    template
}

/// `getNewProfileName`: "Untitled" and one past the highest `Untitled <n>` of the profiles.
fn untitled_name(profiles: &[Profile]) -> String {
    format!("Untitled {}", name_index(profiles.iter().map(|profile| profile.name.as_str()), "Untitled"))
}

/// `getProfileNameIndex`: one past the highest `<n>` of the first `<name>\s(\d+)` in each profile's name.
fn name_index<'a>(names: impl IntoIterator<Item = &'a str>, name: &str) -> u64 {
    let mut highest = 0;
    for profile in names {
        let index = profile.match_indices(name).find_map(|(at, _)| numbered_suffix(&profile[at + name.len()..])).unwrap_or(0);
        highest = highest.max(index);
    }
    highest + 1
}

/// `\s(\d+)` at the start of `rest`.
fn numbered_suffix(rest: &str) -> Option<u64> {
    let mut chars = rest.chars();
    if !chars.next()?.is_whitespace() {
        return None;
    }
    let digits: String = chars.take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

fn set_member(set: &mut BTreeSet<&'static str>, resource: &'static str, member: bool) {
    match member {
        true => set.insert(resource),
        false => set.remove(resource),
    };
}

const IMPORT_URL_HANDLE: i64 = 0;

/// `updateQuickPickItems`: the typed value as a URL to import from, then Select File...
fn import_items(value: &str) -> Vec<Entry> {
    let mut items = Vec::new();
    if !value.is_empty() {
        items.push(Entry::Item(Item { handle: IMPORT_URL_HANDLE, label: value.to_owned(), description: Some("Import from URL".into()), pickable: true, ..Item::default() }));
    }
    items.push(Entry::Item(Item { handle: 1, label: "Select File...".into(), pickable: true, ..Item::default() }));
    items
}

/// `DEFAULT_ICON`: a profile without an icon shows the settings gear.
#[cfg(test)]
mod tests;
