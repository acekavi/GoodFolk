// Fails the build if the browser's bundle holds the SSR secret: its name, or its value when one is set at build time.
import { Glob } from 'bun';

const secret = process.env.IBE_SSR_SECRET;
const needles = ['IBE_SSR_SECRET', ...(secret ? [secret] : [])];
const found: string[] = [];
for await (const path of new Glob('**/*').scan({ cwd: 'build/client', onlyFiles: true })) {
	const text = await Bun.file(`build/client/${path}`).text();
	if (needles.some((needle) => text.includes(needle))) found.push(path);
}
if (found.length) {
	console.error(`The SSR secret is in the browser's bundle: ${found.join(', ')}`);
	process.exit(1);
}
console.log('The browser bundle holds no SSR secret.');
