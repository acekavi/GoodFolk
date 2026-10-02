import { describe, expect, it } from 'vitest';
import * as generated from './gql/graphql';
import persisted from './gql/persisted-documents.json';

const documents = (Object.entries(generated) as [string, unknown][]).filter(
	(entry): entry is [string, { __meta__: { hash: string } }] =>
		entry[1] instanceof generated.TypedDocumentString
);

describe('the persisted-document allowlist', () => {
	it('holds every document the app sends', () => {
		expect(documents.length).toBeGreaterThan(0);
		const missing = documents
			.filter(([, document]) => !(document.__meta__?.hash in persisted))
			.map(([name]) => name);
		// A document edited without `bun run codegen` is sent by a hash the production API does not know: a 400.
		expect(missing, 'stale persisted-documents.json: run `bun run codegen`').toEqual([]);
	});
});
