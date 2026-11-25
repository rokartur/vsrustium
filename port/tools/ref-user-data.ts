// User data of the reference runs: frozen settings and the product's storage, shared by ref-screenshot and ui-pair.
import { Database } from 'bun:sqlite';
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

export function referenceUserData(theme: string, extraSettings: Record<string, unknown> = {}) {
	const userData = mkdtempSync(join(tmpdir(), 'ref-shot-'));
	mkdirSync(join(userData, 'User'));
	// Frozen state: no blinking cursor, no animations, no startup editor, no update or tips noise.
	writeFileSync(join(userData, 'User/settings.json'), JSON.stringify({
		'workbench.colorTheme': theme,
		'workbench.startupEditor': 'none',
		'workbench.reduceMotion': 'on',
		'editor.cursorBlinking': 'solid',
		'window.restoreWindows': 'none',
		'workbench.tips.enabled': false,
		'update.mode': 'none',
		// The port ships without AI and sign-in (port/notes.md); this is the reference look of that product.
		'chat.disableAIFeatures': true,
		// Off as in the reference's integration tests (--disable-workspace-trust); trust checks turn it on with SETTINGS.
		'security.workspace.trust.enabled': false,
		// With AI off the secondary side bar only hosts chat, so the product without AI has none.
		'workbench.secondarySideBar.defaultVisibility': 'hidden',
		...extraSettings,
	}));
	// Same product as the port: no accounts icon (profile storage key of globalCompositeBar.ts isAccountsActionVisible).
	mkdirSync(join(userData, 'User/globalStorage'));
	const storage = new Database(join(userData, 'User/globalStorage/state.vscdb'));
	storage.run('CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB)');
	storage.run(`INSERT INTO ItemTable VALUES ('workbench.activity.showAccounts', 'false')`);
	storage.close();
	return userData;
}

// vscode-api-tests adds the "Hello" status item; product builds exclude it (build/lib/extensions.ts excludedExtensions).
// The port renders sRGB; without the color profile flag Chromium converts to the main display's profile.
export const referenceArgs = ['--force-color-profile=srgb', '--disable-extension', 'GitHub.copilot-chat', '--disable-extension', 'vscode.vscode-api-tests'];
