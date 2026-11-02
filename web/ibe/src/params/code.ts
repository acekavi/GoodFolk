import type { ParamMatcher } from '@sveltejs/kit';

/**
 * A property code (as core-api takes it: 2 to 10 letters and digits), in any case. Anything else, such as
 * `/favicon.ico`, finds no page without asking the API.
 */
export const match: ParamMatcher = (param) => /^[A-Za-z0-9]{2,10}$/.test(param);
