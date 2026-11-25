module.exports = function (grunt) {
	grunt.registerTask('hello', 'Says hello from grunt.', function () {
		grunt.log.writeln('hello from grunt');
	});
	grunt.registerTask('default', ['hello']);
};
