# Profiles editor

## Milestone 10 item 3

Reference: 1.147.0 dev build on the Studio; port: release `vsrustium`. Both driven by `ui-pair` (cua-driver) with fixture `port/fixtures/workspace`, Dark Modern, 1024x768 at 2x, steps `open profiles ui` from the Command Palette. Reference frames in `/tmp/m10-profiles-ref`, port frames in `/tmp/m10-profiles-port` on the Studio.

- The modal part (Profiles title, no JSON action), the New Profile button with its dropdown, the Default row with "Active", the Built-in title, Use for New Windows, the Contents tree (Settings, Keyboard Shortcuts, Tasks, MCP Servers, Snippets, Extensions) and the Folders & Workspaces table with Add Folder match the reference in place and text.
- First port frame differences, fixed in `fix(profiles): match the Profiles editor's row actions and table`: Add Folder stretched to the full width, the Path column started 35pt early, the active row lacked the Open in New Window action.
- Left: text rendering and the 2-4pt vertical offsets of the descriptions (line height of `.profile-description-element`).
- New profile form (steps scratch-only: open profiles ui, click New Profile at (233, 165); frames /tmp/m10-new on the Studio): the Contents description with its Default and None bullets, the Contents and Source header and the Default/None radios match the reference in place and color. After that frame the rows went from 28 to 24pt and the bullets 11pt left, to the reference's measures; not rerun.
- Left in the form: the reference names the profile "Untitled 1", shows the Icon row (picker not ported), keeps Cancel, Preview and Create pinned under a scrolling body, and marks the new sidebar row with a dirty dot and a delete action; the port shows Create and Cancel after the contents and selects the name.
- Gaps (ledger row `workbench/contrib/userDataProfile`): the icon picker, the form differences above.

## Milestone 10 handoff item 1: icon picker and new profile form

Same setup; steps `open profiles ui`, click New Profile at (225, 165) (scratch-only steps; frames in `/tmp/m10-icon` on the Studio).

- First frame after `feat(profiles): add the profile icon picker`: "Untitled 1", the Icon row with the gear and its description, Copy from, the Contents table, Cancel/Preview/Create pinned under the scrolling body and the sidebar row with the dirty dot and trash action match the reference in place and text.
- Differences in that frame, fixed after it: the port's title was italic, the Name input took the focus with its text selected (the reference focuses the form's tree), the new profile's sidebar row used the active selection color (the reference's list is unfocused: inactive selection), the Default row showed its actions without being selected or hovered, and Cancel/Preview lacked the secondary button border. The Copy from select was 26pt high with a gap above it; it is now the select box's 20pt with 2pt corners, 5pt under its description.
- Second frame after those fixes: the form matches the reference in place, text and color, the sidebar shows the Default row without its actions and the new row in the inactive selection. Left: the same 1-3pt vertical offsets of each row as the earlier frames (label and description line heights), the Name input's 2pt corners.
