// The language detection worker (languageDetectionWebWorker.ts `getModelOperations` and `runModel`): one JSON
// request per line on stdin, `{ id, content }`, one answer per line on stdout, `{ id, results }` or `{ id, error }`.
const root = process.env.VSCODE_LANGUAGE_DETECTION_ROOT;
const { ModelOperations } = await import(root + '/dist/lib/index.js');
const operations = new ModelOperations({
	modelJsonLoaderFunc: async () => JSON.parse(await Bun.file(root + '/model/model.json').text()),
	weightsLoaderFunc: async () => await Bun.file(root + '/model/group1-shard1of1.bin').arrayBuffer(),
});
let buffered = '';
for await (const chunk of Bun.stdin.stream()) {
	buffered += new TextDecoder().decode(chunk);
	let newline;
	while ((newline = buffered.indexOf('\n')) >= 0) {
		const { id, content } = JSON.parse(buffered.slice(0, newline));
		buffered = buffered.slice(newline + 1);
		try {
			const results = await operations.runModel(content);
			process.stdout.write(JSON.stringify({ id, results }) + '\n');
		} catch (error) {
			process.stdout.write(JSON.stringify({ id, error: String(error) }) + '\n');
		}
	}
}
