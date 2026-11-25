function main() {
	const greeting = 'hello';
	const numbers = [1, 2, 3];
	const total = numbers.reduce((sum, value) => sum + value, 0);
	console.log(greeting, total);
}
main();
