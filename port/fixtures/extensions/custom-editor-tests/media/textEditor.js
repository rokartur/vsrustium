(function () {
	const vscode = acquireVsCodeApi();
	const textArea = document.querySelector('textarea');

	const initialState = vscode.getState();
	if (initialState) {
		textArea.value = initialState.value;
	}

	window.addEventListener('message', e => {
		switch (e.data.type) {
			case 'fakeInput':
				textArea.value = e.data.value;
				onInput();
				break;
			case 'setValue':
				textArea.value = e.data.value;
				vscode.setState({ value: e.data.value });
				vscode.postMessage({ type: 'didChangeContent', value: e.data.value });
				break;
		}
	});

	const onInput = () => {
		const value = textArea.value;
		vscode.setState({ value });
		vscode.postMessage({ type: 'edit', value });
		vscode.postMessage({ type: 'didChangeContent', value });
	};

	textArea.addEventListener('input', onInput);
}());
