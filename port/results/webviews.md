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
