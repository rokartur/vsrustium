desc('Says hello from jake.');
task('hello', function () {
	console.log('hello from jake');
});
task('default', ['hello']);
