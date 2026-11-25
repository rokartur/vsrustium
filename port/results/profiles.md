# Profiles editor

## Milestone 10 item 3

Reference: 1.147.0 dev build on the Studio; port: release `vsrustium`. Both driven by `ui-pair` (cua-driver) with fixture `port/fixtures/workspace`, Dark Modern, 1024x768 at 2x, steps `open profiles ui` from the Command Palette. Reference frames in `/tmp/m10-profiles-ref`, port frames in `/tmp/m10-profiles-port` on the Studio.

- The modal part (Profiles title, no JSON action), the New Profile button with its dropdown, the Default row with "Active", the Built-in title, Use for New Windows, the Contents tree (Settings, Keyboard Shortcuts, Tasks, MCP Servers, Snippets, Extensions) and the Folders & Workspaces table with Add Folder match the reference in place and text.
- First port frame differences, fixed in `fix(profiles): match the Profiles editor's row actions and table`: Add Folder stretched to the full width, the Path column started 35pt early, the active row lacked the Open in New Window action.
- Left: text rendering and the 2-4pt vertical offsets of the descriptions (line height of `.profile-description-element`).
- New profile form (steps scratch-only: open profiles ui, click New Profile at (233, 165); frames /tmp/m10-new on the Studio): the Contents description with its Default and None bullets, the Contents and Source header and the Default/None radios match the reference in place and color. After that frame the rows went from 28 to 24pt and the bullets 11pt left, to the reference's measures; not rerun.
- Left in the form: the reference names the profile "Untitled 1", shows the Icon row (picker not ported), keeps Cancel, Preview and Create pinned under a scrolling body, and marks the new sidebar row with a dirty dot and a delete action; the port shows Create and Cancel after the contents and selects the name.
- Gaps (ledger row `workbench/contrib/userDataProfile`): the icon picker, the form differences above.
