/**
 * SvelteKit's inline boot script starts importing the app as the HTML is parsed. On a fast connection the app runs
 * before the browser presents its first frame, so the page's largest paint (the hero, the first result's photo) waits
 * for the app's scripts; on a phone's connection they share its bandwidth with that photo. `paintFirst` starts the
 * boot once the browser reports its first contentful frame (a `largest-contentful-paint` entry), or, where that is not
 * reported, after the next frame, and in any case within 3 s: the server-rendered page paints, then hydrates. Only the
 * boot's two markers are touched; a page without them (another kit build, or a chunk without the boot) is returned as
 * it is.
 */
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
	if (!html.includes(IMPORTS) || !html.includes(STARTS)) return html;
	return html
		.replace(IMPORTS, `${PAINTED}.then(() => Promise.all([`)
		.replace(STARTS, '])).then(([kit, app]) => {');
}
