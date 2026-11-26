# Webviews, markdown, media preview, Images Preview, Simple Browser, editor insets (milestone 17)

Reference: 1.147.0 dev build on the Studio; port: release `vsrustium`. Pairs by `port/tools/ui-pair` (Dark Modern, `FOLDER=port/fixtures/webviews`), run by the session's `/tmp/m17c-pairs.sh`; shots in `/tmp/m17c-pairs/<pair>/` on the Studio.

## Extension and fixture checks

| check | reference | port |
|---|---|---|
| `FIXTURE=webview-tests port/tools/webview-check` | 7/7 | 7/7 |
| `FIXTURE=markdown-tests port/tools/webview-check` | 5/5 | 5/5 |
| `FIXTURE=editor-inset-tests port/tools/webview-check` | 2/2 | 2/2 |
| `APP=port FIXTURE=simple-browser-tests port/tools/webview-check` | (integrated browser, not run) | 3/3 |
| `port/tools/extension-tests markdown-language-features test-workspace` | 215 passing, 2 pending | 215 passing, 2 pending |

Fixed on the way: the hidden-webview release now happens when the editor hides, not at the next frame (`preserve vscode API state when hidden` timed out); messages posted before the first webview load wait for it; an extension under development is never disabled by an untrusted workspace (the inset fixture's `vscode` failed to load); http frames load in the https host page (the Simple Browser's page was blocked as mixed content).

## Markdown preview

`FILE=preview.md`, steps `port/fixtures/ui-steps/markdown-preview.json` (Cmd+K V, scrolls, Cmd+2, Cmd+F, type `item 2`, Escape). Same: the preview beside the source with the heading, paragraph, image, lists and code block at the reference's sizes; the preview tab's Open Source and split actions; the find widget at the top right with the typed text, closed by Escape. Differences: the port's find selects the first match and scrolls to it while typing (WebKit `window.find`), the reference shot shows no match yet; the source's breadcrumbs lack the `# Preview fixture` symbol (breadcrumbs outline, not this milestone). The scroll steps moved neither app (background wheel events to the webview). One run lost the Cmd+K chord in the port and typed `v`; the rerun opened the preview.

## Media preview

`FILE=image.png` and `FILE=video.mp4`, steps `port/fixtures/ui-steps/media-preview.json`. Same: the image at its size centered, the Whole Image, 320x200 and 7.33KB status entries; the video centered with its file size; the tab with the file's icon and the Explorer selecting the file (both fixed in this session). Differences: the reference draws a breadcrumbs bar with the file and an "Image Preview"/"Video Preview" editor switcher, and a focusBorder outline around the focused preview; the video's native controls are WebKit's play button in the port and Chromium's controls bar in the reference.

## Simple Browser

`ARGS=--extensionDevelopmentPath=port/fixtures/extensions/simple-browser-pair`, steps `port/fixtures/ui-steps/simple-browser.json`. Same: the tab, the navigation bar with back, forward, reload and open-external buttons, the URL, the local page. Difference: the URL input's text is 11px in the port, 13.33px in the reference (WebKit and Chromium resolve `-webkit-small-control` differently).

## Images Preview

`ARGS=--extensionDevelopmentPath=port/fixtures/extensions/image-carousel-pair`, steps `port/fixtures/ui-steps/image-carousel.json`. Same: the image scaled to fit, the `1 / 2` counter, the download button, the thumbnail strip with the selected border, the next item. Differences: the reference opens the carousel in the modal editor part (`RequiresModal`), the port in an editor tab; a video shows its first frame and WebKit's play button in the port, an empty frame with Chromium's controls bar in the reference.

## Milestone 17 rows, second pass (port-m17c-webviews)

Pairs by `port/tools/ui-pair` (Dark Modern, `FOLDER=port/fixtures/webviews` unless named), shots in `/tmp/m17c-pairs/<pair>/` on the Studio.

- Webview panel restore: steps `port/fixtures/ui-steps/webview-panel-restore.json` (no FILE, folder copied to a temp dir; Cmd+P preview.md, Markdown: Open Preview to the Side, Cmd+Q, restart, Reload Window). Same in both apps: the source beside the `[Preview] preview.md` tab with the rendered preview, after the restart and after the reload. Fixed on the way: the editor state is saved at shutdown too (a webview's `setState` changes no editor, so the saved state had `state: null` and the markdown preview showed "An unexpected error occurred while restoring the Markdown preview"), and the overview ruler's cursor and decoration marks no longer panic on a canvas of height 0 (`clamp(2, -2)` while the restored window laid out). With a FILE argument neither app restores the workspace's editors (layout.ts `filesToOpenOrCreate`).
- Developer tools: `FILE=preview.md`, steps `webview-devtools.json`. The palette runs Developer: Open Webview Developer Tools. The reference docks the workbench's Chromium devtools in the window; the port opens WebKit's Web Inspector window for each webview (the `windows` step lists `Web Inspector — <origin>.vscode-webview.vscode-cdn.net — index.html`, 1000x650). Fixed: `_WKInspector show` neither connected nor opened a window until the view's preferences had developer extras on.
- Editor type control: `FILE=image.png` and `FILE=preview.md`, steps `editor-type-reopen.json`. Same: "Image Preview" with a chevron at the right of the breadcrumbs; its menu with Image Preview (checked), Text Editor, a separator and `Set Default for '*.{jpg,jpe,jpeg,png,bmp,gif,ico,webp,avif,svg}'`; "Text Editor" on preview.md once the editor is reactivated. Differences: the reference shows no control on preview.md opened at startup (it updates on editor change only and the markdown custom editors register after that editor opened); the reference's menu sits 5px left of the window's right edge where the port's touches it. Both menus are 558px wide: a submenu row keeps the empty keybinding span's 2em padding and a 73.6px indicator.
- Audio: `FILE=audio.mp3`, steps `media-preview.json`. Same: the breadcrumbs with "Audio Preview", the player centered, the file size status entry. Difference: WebKit's audio controls against Chromium's.
- Images Preview: steps `image-carousel.json` with `FOLDER=port/fixtures/webviews`. Same: the modal editor part over the workbench with the file-media icon, the "Images Preview" title and the maximize and close actions, the carousel inside it. Fixed with it: the modal title is semibold (modalEditorPart.css `.modal-editor-title`) and the grid's webviews hide while the modal is open.
- Screen reader detection: `SETTINGS='{"editor.accessibilitySupport":"auto"}'`, steps `screen-reader-detection.json`. Before the change the reference showed "Screen reader usage detected" under cua-driver and the port did not; now both show it from the first shot. Electron turns accessibility support on for VoiceOver and for an app setting `AXManualAccessibility` or `AXEnhancedUserInterface` on the application, which cua-driver does; the port had followed AccessKit's tree activity.
- Focus outline: steps `media-preview-focus.json`, `FILE=image.png`. Same: the focusBorder outline around the image at open, in the background and after the app comes forward.
- `custom-editor-tests`: 8/8 in both (reference 7 s, port 22 s). The runs that looked hung waited for the Studio's GUI lock behind another session's pixel-check (8 min) or rebuilt the release binary first (15 min); `gui-lock.sh` now prints `gui-lock: waiting for pid <pid> (<command>)` once while it waits.
