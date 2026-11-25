function hello(done) {
	console.log('hello from gulp');
	done();
}
exports.hello = hello;
exports.default = hello;
