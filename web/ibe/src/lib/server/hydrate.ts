/**
 * SvelteKit's inline boot script starts importing the app as the HTML is parsed. On a fast connection the app runs
 * before the browser presents its first frame, so the page's largest paint (the hero, the first result's photo) waits
 * for the app's scripts; on a phone's connection they share its bandwidth with that photo. `paintFirst` starts the
 * boot once the browser reports its first contentful frame (a `largest-contentful-paint` entry), or, where that is not
 * reported, after the next frame, and in any case within 3 s: the server-rendered page paints, then hydrates. Only the
 * boot script is touched, and only when its markers are all there in their order; any other page (another kit build,
 * or a chunk without the boot) is returned as it is.
 */
const OPEN = '<script>';
const CLOSE = '</script>';
/** The boot's first statement, which only the boot script has. */
const ELEMENT = 'document.currentScript.parentElement';
const IMPORTS = 'Promise.all([';
const STARTS = ']).then(([kit, app]) => {';

/** Resolves once the first contentful frame is presented; the boot's imports wait for it. */
const PAINTED = `new Promise((painted) => {
						setTimeout(painted, 3000);
						const types = (typeof PerformanceObserver === 'function' && PerformanceObserver.supportedEntryTypes) || [];
						if (!types.includes('largest-contentful-paint')) return requestAnimationFrame(() => setTimeout(painted));
						new PerformanceObserver((_, seen) => {
							seen.disconnect();
							setTimeout(painted);
						}).observe({ type: 'largest-contentful-paint', buffered: true });
					})`;

export function paintFirst(html: string): string {
	const boot = bootScript(html);
	if (!boot) return html;
	const { start, end } = boot;
	const script = html.slice(start, end);
	const element = script.indexOf(ELEMENT);
	const imports = script.indexOf(IMPORTS, element);
	const starts = script.indexOf(STARTS, imports + IMPORTS.length);
	if (element === -1 || imports === -1 || starts === -1) return html;
	const patched =
		script.slice(0, imports) +
		`${PAINTED}.then(() => ${IMPORTS}` +
		script.slice(imports + IMPORTS.length, starts) +
		'])).then(([kit, app]) => {' +
		script.slice(starts + STARTS.length);
	return html.slice(0, start) + patched + html.slice(end);
}

/**
 * Where the boot script is: the last bare `<script>` reading `document.currentScript.parentElement`, from its tag to its
 * closing tag. Page content never holds a literal `<script>` (the hotel's text is escaped, `<` as `&lt;` in the HTML
 * and `\u003C` in the serialized data), so content that happens to contain the markers is never touched.
 */
function bootScript(html: string): { start: number; end: number } | null {
	for (
		let start = html.lastIndexOf(OPEN);
		start !== -1;
		start = html.lastIndexOf(OPEN, start - 1)
	) {
		const end = html.indexOf(CLOSE, start);
		if (end === -1) return null;
		if (html.slice(start, end).includes(ELEMENT)) return { start, end };
		if (start === 0) break;
	}
	return null;
}
