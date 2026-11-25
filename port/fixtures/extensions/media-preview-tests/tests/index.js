// Media preview checks run by `FIXTURE=media-preview-tests port/tools/webview-check`: the built-in image, video and
// audio previews open in place of the text editor, as mediaPreview.contribution.ts registers them.
const assert = require('assert');
const vscode = require('vscode');

// A 2x1 PNG.
const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAYAAAD0In+KAAAAEUlEQVR4nGP4z8DwHwyBDAA9xwT8zt1Z8wAAAABJRU5ErkJggg==', 'base64');

const file = name => vscode.Uri.joinPath(vscode.workspace.workspaceFolders[0].uri, name);

const activeTab = () => vscode.window.tabGroups.activeTabGroup.activeTab;

async function until(what, predicate) {
	for (let waited = 0; waited < 15000; waited += 100) {
		const value = predicate();
		if (value) {
			return value;
		}
		await new Promise(resolve => setTimeout(resolve, 100));
	}
	throw new Error(`timed out waiting for ${what}`);
}

async function opensWith(name, bytes, viewType) {
	const uri = file(name);
	await vscode.workspace.fs.writeFile(uri, bytes);
	await vscode.commands.executeCommand('vscode.open', uri);
	const tab = await until(`the ${viewType} tab`, () => {
		const tab = activeTab();
		return tab?.input instanceof vscode.TabInputCustom && tab;
	});
	assert.strictEqual(tab.input.viewType, viewType);
	assert.strictEqual(tab.input.uri.toString(), uri.toString());
	assert.strictEqual(tab.label, name);
	return uri;
}

const checks = {
	async 'an image opens in the image preview'() {
		await opensWith('image.png', png, 'imagePreview.previewEditor');
	},

	async 'a video opens in the video preview'() {
		await opensWith('video.mp4', Buffer.alloc(16), 'vscode.videoPreview');
	},

	async 'an audio file opens in the audio preview'() {
		await opensWith('audio.wav', Buffer.alloc(16), 'vscode.audioPreview');
	},

	async 'the image preview reopens as text and back'() {
		const uri = await opensWith('icon.svg', Buffer.from('<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"/>'), 'imagePreview.previewEditor');
		await vscode.commands.executeCommand('imagePreview.reopenAsText');
		const text = await until('the text tab', () => {
			const tab = activeTab();
			return tab?.input instanceof vscode.TabInputText && tab;
		});
		assert.strictEqual(text.input.uri.toString(), uri.toString());
		assert.strictEqual(vscode.window.tabGroups.activeTabGroup.tabs.length, 1);
		await vscode.commands.executeCommand('imagePreview.reopenAsPreview');
		await until('the preview tab', () => activeTab()?.input instanceof vscode.TabInputCustom);
		assert.strictEqual(vscode.window.tabGroups.activeTabGroup.tabs.length, 1);
	},

	async 'vscode.openWith default opens an image as text'() {
		const uri = file('image.png');
		await vscode.workspace.fs.writeFile(uri, png);
		await vscode.commands.executeCommand('vscode.openWith', uri, 'default');
		await until('the text tab', () => activeTab()?.input instanceof vscode.TabInputText);
	},

	async 'the zoom commands run on the image preview'() {
		await opensWith('image.png', png, 'imagePreview.previewEditor');
		await vscode.commands.executeCommand('imagePreview.zoomIn');
		await vscode.commands.executeCommand('imagePreview.zoomOut');
	},
};

exports.run = async () => {
	const only = process.env.PORT_WEBVIEW_GREP;
	let failed = 0;
	for (const [name, check] of Object.entries(checks)) {
		if (only && !name.includes(only)) {
			continue;
		}
		try {
			await check();
			console.log(`ok ${name}`);
		} catch (error) {
			failed++;
			console.log(`FAILED ${name}: ${error.message}`);
		}
		await vscode.commands.executeCommand('workbench.action.closeAllEditors');
	}
	if (failed) {
		throw new Error(`${failed} media preview checks failed`);
	}
};
