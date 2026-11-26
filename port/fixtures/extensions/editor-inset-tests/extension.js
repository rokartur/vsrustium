// extHostExtensionService.ts maps a `vscode` require to its extension only when the extension has an entry point.
// The tests call the proposed API through this module's `vscode`, the one that carries `enabledApiProposals`.
exports.activate = () => require('vscode');
