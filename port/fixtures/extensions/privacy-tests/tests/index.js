// Telemetry stays inert end to end (roadmap section 19): the API reports telemetry off, a telemetry logger never
// reaches its sender, and the telemetry and experimentation packages resolve to the host's inert modules.
const assert = require('assert');
const vscode = require('vscode');

const checks = {
	'env reports telemetry off'() {
		assert.strictEqual(vscode.env.isTelemetryEnabled, false);
		assert.strictEqual(typeof vscode.env.onDidChangeTelemetryEnabled, 'function');
	},
	async 'a telemetry logger never calls its sender'() {
		const sent = [];
		const logger = vscode.env.createTelemetryLogger({ sendEventData: (name, data) => sent.push(name), sendErrorData: error => sent.push(String(error)) });
		assert.strictEqual(logger.isUsageEnabled, false);
		assert.strictEqual(logger.isErrorsEnabled, false);
		logger.logUsage('usage', { a: 1 });
		logger.logError('error', { a: 1 });
		logger.logError(new Error('boom'));
		await new Promise(resolve => setTimeout(resolve, 100));
		assert.deepStrictEqual(sent, []);
		logger.dispose();
	},
	'@vscode/extension-telemetry is inert through require'() {
		const module = require('@vscode/extension-telemetry');
		for (const Reporter of [module.default, module.TelemetryReporter]) {
			const reporter = new Reporter('InstrumentationKey=00000000-0000-0000-0000-000000000000;IngestionEndpoint=https://example.invalid/');
			assert.strictEqual(reporter.telemetryLevel, 'off');
			reporter.sendTelemetryEvent('event', { a: 'b' }, { c: 1 });
			reporter.sendTelemetryErrorEvent('error');
			reporter.onDidChangeTelemetryLevel(() => { }).dispose();
		}
	},
	async '@vscode/extension-telemetry is inert through import'() {
		const { TelemetryReporter } = await import('./esm.mjs');
		assert.strictEqual(new TelemetryReporter('key').telemetryLevel, 'off');
	},
	async 'vscode-tas-client assigns no experiments'() {
		const tas = require('vscode-tas-client');
		const service = tas.getExperimentationService('privacy-tests', '0.0.1', tas.TargetPopulation.Public, { setSharedProperty() { }, postEvent() { } }, { get() { }, update() { } });
		await service.initialFetch;
		assert.strictEqual(await service.getTreatmentVariableAsync('vscode', 'flag', true), undefined);
		assert.strictEqual(service.isFlightEnabled('flight'), false);
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
