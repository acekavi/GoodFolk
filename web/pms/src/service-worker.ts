/// <reference types="@sveltejs/kit" />
/// <reference no-default-lib="true"/>
/// <reference lib="esnext" />
/// <reference lib="webworker" />

/**
 * Makes the housekeeper app (`/hk`) installable and openable offline by caching the app shell: the build's
 * files and `static`, under a cache named for this build. Only those, plus the SPA fallback page for `/hk`
 * navigations, are ever cached. API and GraphQL responses never are: the app keeps its own saved snapshot of
 * the data it needs. Everything else (the staff app's pages, every request that is not a GET) goes to the
 * network untouched.
 */
import { build, files, version } from '$service-worker';

const sw = self as unknown as ServiceWorkerGlobalScope;

const CACHE = `hk-shell-${version}`;
// The SPA fallback page, which the server answers for every `/hk` route; it is kept under one key.
const SHELL = '/hk';
const ASSETS = new Set([...build, ...files]);

sw.addEventListener('install', (event) => {
	event.waitUntil(
		caches.open(CACHE).then(async (cache) => {
			await cache.addAll([...ASSETS]);
			// Skip the HTTP cache, so the shell matches the build that is being installed.
			await cache.add(new Request(SHELL, { cache: 'reload' }));
		})
	);
});

sw.addEventListener('activate', (event) => {
	event.waitUntil(
		caches
			.keys()
			.then((keys) =>
				Promise.all(keys.filter((key) => key !== CACHE).map((key) => caches.delete(key)))
			)
	);
});

sw.addEventListener('fetch', (event) => {
	const { request } = event;
	if (request.method !== 'GET') return;
	const url = new URL(request.url);
	if (url.origin !== sw.location.origin) return;

	if (request.mode === 'navigate' && (url.pathname === '/hk' || url.pathname.startsWith('/hk/'))) {
		event.respondWith(shell(request));
	} else if (ASSETS.has(url.pathname)) {
		event.respondWith(asset(request));
	}
});

/** The SPA page, from the cache, falling back to the network (and to the network's own error). */
async function shell(request: Request): Promise<Response> {
	return (await caches.match(SHELL, { cacheName: CACHE })) ?? fetch(request);
}

async function asset(request: Request): Promise<Response> {
	return (await caches.match(request, { cacheName: CACHE })) ?? fetch(request);
}
