/*---------------------------------------------------------------------------------------------
 *  Copyright (c) Microsoft Corporation. All rights reserved.
 *  Licensed under the MIT License. See License.txt in the project root for license information.
 *--------------------------------------------------------------------------------------------*/

import * as performance from '../../../base/common/performance.js';
import type * as vscode from 'vscode';
import { createApiFactoryAndRegisterActors } from '../common/extHost.api.impl.js';
import { INodeModuleFactory, RequireInterceptor } from '../common/extHostRequireInterceptor.js';
import { ExtensionActivationTimesBuilder } from '../common/extHostExtensionActivator.js';
import { connectProxyResolver } from './proxyResolver.js';
import { AbstractExtHostExtensionService } from '../common/extHostExtensionService.js';
import { ExtHostDownloadService } from './extHostDownloadService.js';
import { URI } from '../../../base/common/uri.js';
import { Schemas } from '../../../base/common/network.js';
import { IExtensionDescription } from '../../../platform/extensions/common/extensions.js';
import { ExtensionRuntime } from '../common/extHostTypes.js';
import { CLIServer } from './extHostCLIServer.js';
import { realpathSync } from '../../../base/node/pfs.js';
import { ExtHostConsoleForwarder } from './extHostConsoleForwarder.js';
import { ExtHostDiskFileSystemProvider } from './extHostDiskFileSystemProvider.js';
import nodeModule from 'node:module';
import { readFileSync } from 'node:fs';
import { escapeRegExpCharacters } from '../../../base/common/strings.js';
import { assertType } from '../../../base/common/types.js';
import { generateUuid } from '../../../base/common/uuid.js';
import { BidirectionalMap } from '../../../base/common/map.js';
import { DisposableStore, IDisposable, toDisposable } from '../../../base/common/lifecycle.js';
import { ExtensionDescriptionRegistry } from '../../services/extensions/common/extensionDescriptionRegistry.js';
import { markNodeCompileCacheReady } from '../../../base/node/nodeCompileCache.js';
import { inertModuleExports, inertModuleNames, inertModuleSource } from '../common/extHostInertModules.js';
const require = nodeModule.createRequire(import.meta.url);

class NodeModuleRequireInterceptor extends RequireInterceptor {

	private static _createDataUri(scriptContent: string): string {
		return `data:text/javascript;base64,${Buffer.from(scriptContent).toString('base64')}`;
	}

	private static _vscodeImportFnName = `_VSCODE_IMPORT_VSCODE_API`;
	private static _inertImportFnName = `_VSCODE_IMPORT_INERT_MODULE`;

	private readonly _store = new DisposableStore();

	dispose(): void {
		this._store.dispose();
	}

	protected _installInterceptor(): void {
		const that = this;
		const node_module = require('module');
		const originalLoad = node_module._load;
		node_module._load = function load(request: string, parent: { filename: string }, isMain: boolean) {
			request = applyAlternatives(request);
			if (!that._factories.has(request)) {
				return originalLoad.apply(this, arguments);
			}
			return that._factories.get(request)!.load(
				request,
				URI.file(realpathSync(parent.filename)),
				request => originalLoad.apply(this, [request, parent, isMain])
			);
		};

		const originalLookup = node_module._resolveLookupPaths;
		node_module._resolveLookupPaths = (request: string, parent: unknown) => {
			return originalLookup.call(this, applyAlternatives(request), parent);
		};

		// Bun's require() skips Module._load, so on Bun the factories are served from here through require.cache.
		const bunModuleKeys = new Map<unknown, string>();
		const originalResolveFilename = node_module._resolveFilename;
		node_module._resolveFilename = function resolveFilename(request: string, parent: { filename: string } | undefined, isMain: boolean, options?: { paths?: string[] }) {
			if (process.versions.bun) {
				request = applyAlternatives(request);
				const factory = that._factories.get(request);
				if (factory && parent?.filename) {
					const exports = factory.load(request, URI.file(realpathSync(parent.filename)), request => originalLoad.apply(this, [request, parent, isMain]));
					let key = bunModuleKeys.get(exports);
					if (!key) {
						key = `/__vscode_module__/${generateUuid()}/${request}`;
						bunModuleKeys.set(exports, key);
						node_module._cache[key] = { id: key, filename: key, loaded: true, exports };
					}
					return key;
				}
			}
			if (request === 'vsda' && Array.isArray(options?.paths) && options.paths.length === 0) {
				// ESM: ever since we moved to ESM, `require.main` will be `undefined` for extensions
				// Some extensions have been using `require.resolve('vsda', { paths: require.main.paths })`
				// to find the `vsda` module in our app root. To be backwards compatible with this pattern,
				// we help by filling in the `paths` array with the node modules paths of the current module.
				options.paths = node_module._nodeModulePaths(import.meta.dirname);
			}
			return originalResolveFilename.call(this, request, parent, isMain, options);
		};

		const applyAlternatives = (request: string) => {
			for (const alternativeModuleName of that._alternatives) {
				const alternative = alternativeModuleName(request);
				if (alternative) {
					request = alternative;
					break;
				}
			}
			return request;
		};

		const apiInstances = new BidirectionalMap<typeof vscode, string>();
		const apiImportDataUrl = new Map<string, string>();

		// define a global function that can be used to get API instances given a random key
		Object.defineProperty(globalThis, NodeModuleRequireInterceptor._vscodeImportFnName, {
			enumerable: false,
			configurable: false,
			writable: false,
			value: (key: string) => {
				return apiInstances.getKey(key);
			}
		});

		let apiModuleFactory: INodeModuleFactory | undefined;

		const lookup = (url: string): string => {
			// Get the vscode-module factory - which is the same logic that's also used by
			// the CommonJS require interceptor
			if (!apiModuleFactory) {
				apiModuleFactory = this._factories.get('vscode');
				assertType(apiModuleFactory);
			}

			const uri = URI.parse(url);

			// Get or create the API instance. The interface is per extension and extensions are
			// looked up by the uri (e.data.url) and path containment.
			const apiInstance = apiModuleFactory.load('_not_used', uri, () => { throw new Error('CANNOT LOAD MODULE from here.'); });
			let key = apiInstances.get(apiInstance);
			if (!key) {
				key = generateUuid();
				apiInstances.set(apiInstance, key);
			}

			// Create and cache a data-url which is the import script for the API instance
			let scriptDataUrlSrc = apiImportDataUrl.get(key);
			if (!scriptDataUrlSrc) {
				const jsCode = `const _vscodeInstance = globalThis.${NodeModuleRequireInterceptor._vscodeImportFnName}('${key}');\n\n${Object.keys(apiInstance).map((name => `export const ${name} = _vscodeInstance['${name}'];`)).join('\n')}`;
				scriptDataUrlSrc = NodeModuleRequireInterceptor._createDataUri(jsCode);
				apiImportDataUrl.set(key, scriptDataUrlSrc);
			}
			return scriptDataUrlSrc;
		};

		Object.defineProperty(globalThis, NodeModuleRequireInterceptor._inertImportFnName, {
			enumerable: false,
			configurable: false,
			writable: false,
			value: inertModuleExports
		});
		const inertDataUrls = new Map(inertModuleNames.map(name => [name, NodeModuleRequireInterceptor._createDataUri(inertModuleSource(name, NodeModuleRequireInterceptor._inertImportFnName))]));
		const resolveImport = (specifier: string, parentUrl: string) => specifier === 'vscode' ? lookup(parentUrl) : inertDataUrls.get(specifier);

		if (process.versions.bun) {
			this._store.add(installBunESMInterceptor(this._extensionRegistry.all, resolveImport));
			return;
		}
		const hooks = nodeModule.registerHooks({
			resolve: (specifier, context, nextResolve) => {
				const otherUrl = context.parentURL && resolveImport(specifier, context.parentURL);
				if (!otherUrl) {
					return nextResolve(specifier, context);
				}
				return {
					url: otherUrl,
					shortCircuit: true,
				};
			},
		});
		this._store.add(toDisposable(() => hooks.deregister()));
	}
}

declare const Bun: {
	plugin(plugin: { name: string; setup(build: { onLoad(options: { filter: RegExp }, load: (args: { path: string }) => { contents: string; loader: 'js' }): void }): void }): void;
};

/**
 * Bun 1.4.5 has no `module.registerHooks` and its runtime `onResolve` does not see bare specifiers,
 * so ESM extensions get their `vscode` (and inert module) imports rewritten to the same data: URL the Node hook resolves to.
 */
function installBunESMInterceptor(registry: ExtensionDescriptionRegistry, lookup: ImportResolver): IDisposable {
	const coveredRoots = new Set<string>();
	const newESMRoots = () => {
		// Bun matches the resolved path: /var/folders is /private/var/folders on macOS.
		const roots = registry.getAllExtensionDescriptions().filter(extension => extension.type === 'module').map(extension => realpathSync(extension.extensionLocation.fsPath)).filter(root => !coveredRoots.has(root));
		roots.forEach(root => coveredRoots.add(root));
		return roots.map(escapeRegExpCharacters);
	};
	const esmRoots = newESMRoots();
	addVSCodeImportPlugin(esmRoots.length ? new RegExp(`\\.mjs$|${esmRootsPattern(esmRoots)}`) : /\.mjs$/, lookup);
	// Bun.plugin filters are fixed once registered: ESM extensions added through $deltaExtensions get a plugin of their own.
	return registry.onDidChange(() => {
		const added = newESMRoots();
		if (added.length) {
			addVSCodeImportPlugin(new RegExp(esmRootsPattern(added)), lookup);
		}
	});
}

function esmRootsPattern(escapedRoots: string[]): string {
	return `^(?:${escapedRoots.join('|')})/(?!.*/node_modules/).*\\.js$`;
}

type ImportResolver = (specifier: string, parentUrl: string) => string | undefined;

function addVSCodeImportPlugin(filter: RegExp, lookup: ImportResolver): void {
	const specifiers = ['vscode', ...inertModuleNames].map(escapeRegExpCharacters).join('|');
	const vscodeImport = new RegExp(`(\\bfrom\\s*|\\bimport\\s*\\(\\s*|\\bimport\\s+)(['"])(${specifiers})\\2`, 'g');
	Bun.plugin({
		name: 'vscode-api',
		setup(build) {
			// onLoad must stay synchronous: an async onLoad turns every require() of a matched file into an error.
			build.onLoad({ filter }, ({ path }) => {
				const source = readFileSync(path, 'utf8');
				const contents = source.includes('vscode') ? source.replace(vscodeImport, (_match, prefix: string, _quote: string, specifier: string) => `${prefix}'${lookup(specifier, URI.file(path).toString(true))}'`) : source;
				return { contents, loader: 'js' };
			});
		}
	});
}

export class ExtHostExtensionService extends AbstractExtHostExtensionService {

	readonly extensionRuntime = ExtensionRuntime.Node;

	protected async _beforeAlmostReadyToRunExtensions(): Promise<void> {
		// make sure console.log calls make it to the render
		this._instaService.createInstance(ExtHostConsoleForwarder);

		// initialize API and register actors
		const extensionApiFactory = this._instaService.invokeFunction(createApiFactoryAndRegisterActors);

		// Register Download command
		this._instaService.createInstance(ExtHostDownloadService);

		// Register CLI Server for ipc
		if (this._initData.remote.isRemote && this._initData.remote.authority) {
			const cliServer = this._instaService.createInstance(CLIServer);
			process.env['VSCODE_IPC_HOOK_CLI'] = cliServer.ipcHandlePath;
		}

		// Register local file system shortcut
		this._instaService.createInstance(ExtHostDiskFileSystemProvider);

		// Module loading tricks based on `module._load`.
		// `module._load` intercepts `require(...)`.
		// Module loading tricks based on `module.registerHooks`.
		// `module.registerHooks` is a generic interceptor that intercepts `require(...)`, `import ...`, and `import(...)`.
		await this._store.add(this._instaService.createInstance(NodeModuleRequireInterceptor, extensionApiFactory, { mine: this._myRegistry, all: this._globalRegistry }))
			.install();

		performance.mark('code/extHost/didInitAPI');

		// Do this when extension service exists, but extensions are not being activated yet.
		const configProvider = await this._extHostConfiguration.getConfigProvider();
		await connectProxyResolver(this._extHostWorkspace, configProvider, this, this._logService, this._mainThreadTelemetryProxy, this._initData, this._store);
		performance.mark('code/extHost/didInitProxyResolver');
	}

	protected _getEntryPoint(extensionDescription: IExtensionDescription): string | undefined {
		return extensionDescription.main;
	}

	private async _doLoadModule<T>(extension: IExtensionDescription | null, module: URI, activationTimesBuilder: ExtensionActivationTimesBuilder, mode: 'esm' | 'cjs'): Promise<T> {
		if (module.scheme !== Schemas.file) {
			throw new Error(`Cannot load URI: '${module}', must be of file-scheme`);
		}
		let r: T | null = null;
		activationTimesBuilder.codeLoadingStart();
		this._logService.trace(`ExtensionService#loadModule [${mode}] -> ${module.toString(true)}`);
		this._logService.flush();
		const extensionId = extension?.identifier.value;
		if (extension) {
			await this._extHostLocalizationService.initializeLocalizedMessages(extension);
		}
		try {
			if (extensionId) {
				performance.mark(`code/extHost/willLoadExtensionCode/${extensionId}`);
			}
			if (mode === 'esm') {
				r = <T>await import(module.toString(true));
			} else {
				r = <T>require(module.fsPath);
			}
		} finally {
			if (extensionId) {
				performance.mark(`code/extHost/didLoadExtensionCode/${extensionId}`);
			}
			activationTimesBuilder.codeLoadingStop();
		}
		return r;
	}

	protected async _loadCommonJSModule<T>(extension: IExtensionDescription | null, module: URI, activationTimesBuilder: ExtensionActivationTimesBuilder): Promise<T> {
		return this._doLoadModule<T>(extension, module, activationTimesBuilder, 'cjs');
	}

	protected async _loadESMModule<T>(extension: IExtensionDescription | null, module: URI, activationTimesBuilder: ExtensionActivationTimesBuilder): Promise<T> {
		return this._doLoadModule<T>(extension, module, activationTimesBuilder, 'esm');
	}

	public async $setRemoteEnvironment(env: { [key: string]: string | null }): Promise<void> {
		if (!this._initData.remote.isRemote) {
			return;
		}

		for (const key in env) {
			const value = env[key];
			if (value === null) {
				delete process.env[key];
			} else {
				process.env[key] = value;
			}
		}
	}

	protected override _onEagerExtensionsActivated(): void {
		markNodeCompileCacheReady(message => this._logService.info(message));
	}
}
