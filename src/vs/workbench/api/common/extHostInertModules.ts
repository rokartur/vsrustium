/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See License.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/

import { Event } from '../../../base/common/event.js';
import { URI } from '../../../base/common/uri.js';
import type { INodeModuleFactory } from './extHostRequireInterceptor.js';

// VSRustium sends no telemetry (port/notes.md, milestone 19): the telemetry and experimentation client packages
// built-in extensions require are served as inert modules, so their network code is never loaded.

class InertTelemetryReporter {
	readonly telemetryLevel = 'off';
	readonly onDidChangeTelemetryLevel = Event.None;
	constructor(_connectionString?: string, _replacementOptions?: unknown[]) { }
	sendTelemetryEvent(): void { }
	sendRawTelemetryEvent(): void { }
	sendDangerousTelemetryEvent(): void { }
	sendTelemetryErrorEvent(): void { }
	sendDangerousTelemetryErrorEvent(): void { }
	sendTelemetryException(): void { }
	async dispose(): Promise<void> { }
}

// vscode-tas-client's TelemetryDisabledExperimentationService, what it returns while telemetry is off.
class InertExperimentationService {
	readonly initializePromise = Promise.resolve();
	readonly initialFetch = Promise.resolve();
	isFlightEnabled(): boolean { return false; }
	async isCachedFlightEnabled(): Promise<boolean> { return false; }
	async isFlightEnabledAsync(): Promise<boolean> { return false; }
	getTreatmentVariable(): undefined { return undefined; }
	async getTreatmentVariableAsync(): Promise<undefined> { return undefined; }
}

const inertModules: ReadonlyMap<string, object> = new Map<string, object>([
	// 0.9.x exports the class as `default`, 1.x as `TelemetryReporter`.
	['@vscode/extension-telemetry', { __esModule: true, default: InertTelemetryReporter, TelemetryReporter: InertTelemetryReporter }],
	['vscode-tas-client', {
		__esModule: true,
		getExperimentationService: () => new InertExperimentationService(),
		getExperimentationServiceAsync: async () => new InertExperimentationService(),
		TargetPopulation: { Team: 'team', Internal: 'internal', Insiders: 'insider', Public: 'public' },
	}],
]);

export const inertModuleNames = [...inertModules.keys()];

export function inertModuleExports(name: string): object | undefined {
	return inertModules.get(name);
}

export class InertTelemetryModuleFactory implements INodeModuleFactory {
	readonly nodeModuleName = inertModuleNames;
	load(request: string, _parent: URI): object {
		return inertModules.get(request)!;
	}
}

/** ESM source that re-exports an inert module, for the import hooks. */
export function inertModuleSource(name: string, globalLookupName: string): string {
	const exports = Object.keys(inertModules.get(name)!).filter(key => key !== '__esModule' && key !== 'default');
	const lines = [`const m = globalThis.${globalLookupName}(${JSON.stringify(name)});`, 'export default m.default ?? m;'];
	for (const key of exports) {
		lines.push(`export const ${key} = m[${JSON.stringify(key)}];`);
	}
	return lines.join('\n');
}
