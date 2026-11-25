test('adds', () => 1 + 1);
test('subtracts', () => 2 - 1);
test('fails on purpose', () => { throw new Error('fail'); });
