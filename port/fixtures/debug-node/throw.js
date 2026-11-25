// An uncaught exception in a timer callback: js-debug stops on it and the exception widget shows below line 3.
function parse(text) {
	throw new Error(`cannot parse ${text}`);
}
setTimeout(() => parse('input'), 0);
