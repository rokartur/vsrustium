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
