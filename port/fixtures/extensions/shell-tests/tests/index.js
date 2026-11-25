// Product shell round trips (roadmap section 19): MainThreadPower and MainThreadMeteredConnection answer the host
// with what macOS reports, and the host sees the product's name, URL scheme and version.
const assert = require('assert');
const { execFileSync } = require('child_process');
const { readFileSync } = require('fs');
const { join } = require('path');
const vscode = require('vscode');

const run = (command, ...args) => execFileSync(command, args, { encoding: 'utf8' });
const root = process.env.SHELL_TESTS_ROOT;

const checks = {
	async 'idle time and state follow the session idle time'() {
		// Chromium's idle time: CGEventSourceSecondsSinceLastEventType(kCGEventSourceStateCombinedSessionState, kCGAnyInputEventType).
		const sessionIdle = Number(run('osascript', '-l', 'JavaScript', '-e', "ObjC.import('CoreGraphics'); $.CGEventSourceSecondsSinceLastEventType(0, 0xFFFFFFFF)"));
		const idle = await vscode.env.power.getSystemIdleTime();
		assert.ok(Math.abs(idle - sessionIdle) <= 2, `idle ${idle} s, session ${sessionIdle} s`);
		const locked = run('ioreg', '-n', 'Root', '-d1').includes('"CGSSessionScreenIsLocked"=Yes');
		assert.strictEqual(await vscode.env.power.getSystemIdleState(Math.floor(idle) + 60), locked ? 'locked' : 'active');
		if (!locked && idle >= 2) {
			assert.strictEqual(await vscode.env.power.getSystemIdleState(1), 'idle');
		}
	},
	async 'thermal state is one of the Electron names'() {
		assert.ok(['unknown', 'nominal', 'fair', 'serious', 'critical'].includes(await vscode.env.power.getCurrentThermalState()));
	},
	async 'battery power matches pmset'() {
		assert.strictEqual(await vscode.env.power.isOnBatteryPower(), run('pmset', '-g', 'batt').includes("'Battery Power'"));
	},
	async 'a power save blocker holds an IOPM assertion until disposed'() {
		const blocker = await vscode.env.power.startPowerSaveBlocker('prevent-display-sleep');
		assert.strictEqual(typeof blocker.id, 'number');
		assert.strictEqual(blocker.isStarted, true);
		const assertions = () => run('pmset', '-g', 'assertions').split('\n').filter(line => line.includes('(vsrustium)') && line.includes('NoDisplaySleepAssertion') && line.includes('named: "Electron"'));
		assert.strictEqual(assertions().length, 1, run('pmset', '-g', 'assertions').split('\n').filter(line => line.includes('vsrustium') || line.includes('Electron')).join(' / ') || 'no vsrustium assertion');
		blocker.dispose();
		await new Promise(resolve => setTimeout(resolve, 500));
		assert.strictEqual(assertions().length, 0);
	},
	'metered connection is reported with its event'() {
		assert.strictEqual(typeof vscode.env.isMeteredConnection, 'boolean');
		assert.strictEqual(vscode.env.isMeteredConnection, process.env.SHELL_TESTS_METERED === '1');
		vscode.env.onDidChangeMeteredConnection(() => { }).dispose();
	},
	'product name, URL scheme and version reach the host'() {
		const product = JSON.parse(readFileSync(join(root, 'product.json'), 'utf8'));
		const pkg = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8'));
		assert.strictEqual(vscode.env.appName, product.nameLong);
		assert.strictEqual(vscode.env.uriScheme, product.urlProtocol);
		assert.strictEqual(vscode.version, product.version ?? pkg.version);
	},
};

exports.run = async () => {
	let failed = 0;
	for (const [name, check] of Object.entries(checks)) {
		try {
			await check();
			console.log(`ok ${name}`);
		} catch (error) {
			failed++;
			console.log(`FAILED ${name}: ${error.message}`);
		}
	}
	if (failed) {
		throw new Error(`${failed} failed`);
	}
};
