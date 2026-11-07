// Writes core-api's OpenAPI document cut down to the public API: the `/api/public/` paths and the schemas they
// reference, directly or through other schemas. The site calls nothing else, and the staff API's changes stay out of
// its generated types. Usage: bun scripts/public-openapi.ts <full openapi.json> <output>
type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
type Document = { paths: Record<string, Json>; components?: { schemas?: Record<string, Json> } };

const [from, to] = process.argv.slice(2);
if (!from || !to) throw new Error('usage: bun scripts/public-openapi.ts <openapi.json> <output>');
const document: Document = await Bun.file(from).json();

const paths = Object.fromEntries(
	Object.entries(document.paths).filter(([path]) => path.startsWith('/api/public/'))
);

/** The schema names `value` refers to with `#/components/schemas/<name>`. */
function refs(value: Json, found: string[] = []): string[] {
	if (Array.isArray(value)) value.forEach((item) => refs(item, found));
	else if (value && typeof value === 'object') {
		for (const [key, item] of Object.entries(value)) {
			if (key === '$ref' && typeof item === 'string' && item.startsWith('#/components/schemas/'))
				found.push(item.slice('#/components/schemas/'.length));
			else refs(item, found);
		}
	}
	return found;
}

const all = document.components?.schemas ?? {};
const kept = new Set<string>();
const pending = refs(paths);
while (pending.length) {
	const name = pending.pop()!;
	if (kept.has(name) || !(name in all)) continue;
	kept.add(name);
	pending.push(...refs(all[name]));
}
const schemas = Object.fromEntries([...kept].sort().map((name) => [name, all[name]]));

await Bun.write(
	to,
	JSON.stringify({ ...document, paths, components: { ...document.components, schemas } }, null, 2) +
		'\n'
);
