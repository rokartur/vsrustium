# Settings editor

## Milestone 10 item 1: tree, search, widgets, validation, policies

Reference: 1.147.0 dev build on the Studio; port: release `vsrustium`. Both driven by `ui-pair` (cua-driver) with fixture `port/fixtures/workspace`, Dark Modern, 1024x768 at 2x. Reference frames in `/tmp/se-item1` on the Studio, port frames in `/tmp/se-item1d`; `magick compare -metric AE` against the reference, lower is closer.

| step | before (first pair) | now |
| --- | --- | --- |
| `open` (Cmd+,) | 62276 | 57206 |
| `suggest` (`@` in the search box) | 66169 | 62994 |
| `suggest-accepted` (Down, Return) | 80878 | 76212 |
| `object` (`files exclude`) | 90870 | 66587 |

- The remaining differences are text rendering and the rows the port does not have: settings the reference's built-in extensions register before the window opens (Copilot, GitHub), and the descriptions those change.
- `policy` (port only, `defaults write com.rokartur.vsrustium UpdateMode -string none`, search `update mode`, frame `/tmp/se-item1e/port-policy.png`): Update: Mode shows `none` with the briefcase "Managed by your organization" indicator and a read-only dropdown. ui-pair's `type` step lowercases text after `@`, so `@hasPolicy` is checked in-process instead.
- Tabs use the modern UI override (`modernUI/browser/media/tabs.css` 1153: `padding: 2px 8px`, semibold), not the classic 7px/6.5px padding.

Checks (Studio):

- `cargo test -p app settings_editor`: 47 passed (`a_policy_value_shows_instead_of_the_scope_value_and_is_not_configured`, `filters_match_tags_ids_and_extensions` with `@hasPolicy`).
- `cargo test -p app preferences`: 14 passed (`a_setting_set_by_policy_gets_only_the_policy_hint`).
- `cargo test -p app policy`: 3 passed; `cargo test -p app configuration_resolver`: 1 passed; `cargo test -p config policy`: 13 passed.

## Milestone 11: settings.json in the modal editor group

`port/tools/ui-pair` with `FILE=src/main.ts`, steps `port/fixtures/preferences/steps-json.json` (Command Palette, "open user settings json", Enter); pair `raw/preferences/settings-json-modal-pair.png`, reference left.

Same: settings.json opens in the centered modal over the window with main.ts still in the editor group behind it, the modal's size and place, the header with the file icon, name and folder and its Open Settings (UI), Open in Main Window (disabled in the port), Maximize and Close actions, no tabs, the breadcrumbs above the text, the text's first line at the same height.

Differences: the header's folder is cut with "..." in the reference; the port's file breadcrumbs start at "/" with wider gaps (the port's breadcrumbs, also outside the modal); the status bar follows the modal's editor in the port and the main group's in the reference; the main group's split action is hidden while the modal has the focus; the reference's edit pencil in the glyph margin (settings editor contribution, not ported). `cargo test -p app modal_editor_group`: 2 passed.
