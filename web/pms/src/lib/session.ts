import { rest } from './api/rest';
import { toApiError } from './api/problem';
import type { components } from './api/openapi';

export type Profile = components['schemas']['Profile'];

export async function fetchMe(): Promise<Profile> {
	const { data, error, response } = await rest.GET('/api/v1/me');
	if (!response.ok || !data) throw toApiError(error, response.status);
	return data;
}

/** UI hint only; the API enforces permissions. */
export function isTenantOwner(profile: Profile): boolean {
	return profile.grants.some((grant) => grant.role === 'owner' && grant.property_id == null);
}
