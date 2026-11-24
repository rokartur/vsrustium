const fs = require('fs');
let context;
exports.activate = c => { context = c; };
exports.run = async () => {
	const s = context.secrets, out = [], events = [];
	s.onDidChange(e => events.push(e.key));
	const tick = () => new Promise(r => setTimeout(r, 200));
	out.push(['get missing', await s.get('token')]);
	await s.store('token', 's3cret'); await s.store('other', 'x'); await tick();
	out.push(['get', await s.get('token')], ['keys', (await s.keys()).sort()], ['events', events.splice(0)]);
	await s.store('token', 's3cret'); await tick();
	out.push(['same value events', events.splice(0)]);
	await s.delete('token'); await s.delete('token'); await tick();
	out.push(['after delete', await s.get('token')], ['keys', await s.keys()], ['delete events', events.splice(0)]);
	await s.delete('other');
	fs.writeFileSync(process.env.CHECK_OUT, JSON.stringify(out));
};
