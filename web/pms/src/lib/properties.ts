import { graphql } from './api/gql';
import { query } from './api/graphql';

export const PropertiesDocument = graphql(`
	query Properties {
		properties {
			id
			code
			name
			timezone
			baseCurrency
			businessDate
			checkInTime
			checkOutTime
			version
		}
	}
`);

/** Query key shared with the server's `properties` invalidation event. */
export const propertiesKey = ['properties'] as const;

export async function fetchProperties(signal?: AbortSignal) {
	return (await query(PropertiesDocument, undefined, signal)).properties;
}
