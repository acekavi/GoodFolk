import type { CodegenConfig } from '@graphql-codegen/cli';

const config: CodegenConfig = {
	schema: 'src/lib/api/schema.graphql',
	// Queries live in .ts modules under src/lib, never inline in components.
	documents: ['src/lib/**/*.ts', '!src/lib/api/gql/**'],
	ignoreNoDocuments: true,
	generates: {
		'src/lib/api/gql/': {
			preset: 'client',
			config: {
				// Documents are plain strings: no GraphQL parser is shipped to the browser.
				documentMode: 'string',
				useTypeImports: true,
				scalars: { UUID: 'string', Date: 'string', DateTime: 'string', JSON: 'unknown' }
			}
		}
	}
};

export default config;
