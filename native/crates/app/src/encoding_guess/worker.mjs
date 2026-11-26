// The encoding guess of encoding.ts (`guessEncodingByBuffer`, `guessEncodingByCandidates`) over the app's jschardet
// and iconv-lite: one request per line on stdin, `{ bytes, candidates }` with the bytes as hex, one answer per line
// on stdout, `{ encoding }` (null when there is no guess).
import { createRequire } from 'node:module';

const require = createRequire(process.env.VSCODE_NODE_MODULES + '/');
const jschardet = require('jschardet/dist/jschardet.min.js');
const iconv = require('@vscode/iconv-lite-umd/lib/iconv-lite-umd.js');

const GUESSABLE_ENCODINGS = {
	utf8: 'UTF-8', utf16le: 'UTF-16LE', utf16be: 'UTF-16BE', windows1252: 'windows-1252', windows1250: 'windows-1250',
	iso88592: 'ISO-8859-2', windows1251: 'windows-1251', cp866: 'IBM866', cp1125: 'IBM1125', iso88595: 'ISO-8859-5',
	koi8r: 'KOI8-R', windows1253: 'windows-1253', iso88597: 'ISO-8859-7', windows1255: 'windows-1255', iso88598: 'ISO-8859-8',
	cp950: 'Big5', shiftjis: 'SHIFT_JIS', eucjp: 'EUC-JP', euckr: 'EUC-KR', gb2312: 'GB2312',
};
const JSCHARDET_TO_ICONV_ENCODINGS = { ibm866: 'cp866', big5: 'cp950' };
const IGNORE_ENCODINGS = ['ascii', 'utf-16', 'utf-32'];
const AUTO_ENCODING_GUESS_MAX_BYTES = 512 * 128;

const normalizeEncoding = name => name.replace(/[^a-zA-Z0-9]/g, '').toLowerCase();
const toIconvLiteEncoding = name => JSCHARDET_TO_ICONV_ENCODINGS[normalizeEncoding(name)] || normalizeEncoding(name);
const isUTFEncoding = encoding => ['utf8', 'utf8bom', 'utf16be', 'utf16le'].includes(encoding);

function guessEncodingByBuffer(buffer, candidateGuessEncodings) {
	const limitedBuffer = buffer.subarray(0, AUTO_ENCODING_GUESS_MAX_BYTES);
	const binaryString = limitedBuffer.toString('latin1');
	if (candidateGuessEncodings) {
		candidateGuessEncodings = candidateGuessEncodings.map(e => GUESSABLE_ENCODINGS[normalizeEncoding(e)]).filter(Boolean);
		if (candidateGuessEncodings.length === 0) {
			candidateGuessEncodings = undefined;
		}
	}
	let guessed;
	try {
		guessed = jschardet.detect(binaryString, candidateGuessEncodings ? { detectEncodings: candidateGuessEncodings } : undefined);
	} catch {
		return null;
	}
	if (!guessed?.encoding) {
		return candidateGuessEncodings ? guessEncodingByCandidates(limitedBuffer, candidateGuessEncodings) : null;
	}
	if (IGNORE_ENCODINGS.includes(guessed.encoding.toLowerCase())) {
		return null;
	}
	return toIconvLiteEncoding(guessed.encoding);
}

function guessEncodingByCandidates(buffer, candidates) {
	try {
		new TextDecoder('utf-8', { fatal: true }).decode(buffer, { stream: true });
		return null;
	} catch (error) {
		if (!(error instanceof TypeError)) {
			throw error;
		}
	}
	let guessedEncoding = null;
	for (const candidate of candidates) {
		const encoding = toIconvLiteEncoding(candidate);
		if (isUTFEncoding(encoding)) {
			continue;
		}
		if (!Buffer.from(iconv.encode(iconv.decode(buffer, encoding), encoding)).equals(buffer)) {
			continue;
		}
		if (guessedEncoding && guessedEncoding !== encoding) {
			return null;
		}
		guessedEncoding = encoding;
	}
	return guessedEncoding;
}

let buffered = '';
for await (const chunk of Bun.stdin.stream()) {
	buffered += new TextDecoder().decode(chunk);
	let newline;
	while ((newline = buffered.indexOf('\n')) >= 0) {
		const { bytes, candidates } = JSON.parse(buffered.slice(0, newline));
		buffered = buffered.slice(newline + 1);
		let encoding = null;
		try {
			encoding = guessEncodingByBuffer(Buffer.from(bytes, 'hex'), candidates);
		} catch { }
		process.stdout.write(JSON.stringify({ encoding }) + '\n');
	}
}
