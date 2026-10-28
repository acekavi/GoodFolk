import { rest, unwrap } from './api/rest';
import { mediaKey } from './media';

/** The smallest contrast white text needs against the accent colour; mirrors the server's check. */
export const MIN_ACCENT_CONTRAST = 4.5;

export const ACCENT_TOO_PALE = 'The accent colour needs more contrast with white text';

/** The WCAG contrast ratio of white against `#rrggbb`, or `null` for any other text. */
export function contrastWithWhite(hex: string): number | null {
	const match = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(hex);
	if (!match) return null;
	const [r, g, b] = match.slice(1).map((digits) => {
		const value = parseInt(digits, 16) / 255;
		return value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
	});
	return 1.05 / (0.2126 * r + 0.7152 * g + 0.0722 * b + 0.05);
}

/** Why the server would refuse `hex` as the accent, or `null` if it takes it. */
export function accentProblem(hex: string): string | null {
	const contrast = contrastWithWhite(hex);
	if (contrast === null) return 'Use a colour like #1d4ed8';
	return contrast < MIN_ACCENT_CONTRAST ? ACCENT_TOO_PALE : null;
}

/** Under the server's `media:<property>` event, which the logo finishing processing queues. */
export function ibeSiteKey(propertyId: string) {
	return [...mediaKey(propertyId), 'ibe-site'] as const;
}

export async function fetchIbeSite(propertyId: string, signal?: AbortSignal) {
	return unwrap(
		await rest.GET('/api/v1/properties/{property}/ibe-site', {
			params: { path: { property: propertyId } },
			signal
		})
	);
}

/** The origin of the public booking site (`IBE_PUBLIC_ORIGIN`), given to the build as `VITE_IBE_PUBLIC_ORIGIN`. */
export const IBE_PUBLIC_ORIGIN: string = import.meta.env.VITE_IBE_PUBLIC_ORIGIN ?? '';

/** The booking site's address for a property under `origin`; `null` when the build knows no origin. */
export function previewUrl(code: string, origin: string = IBE_PUBLIC_ORIGIN): string | null {
	return origin ? `${origin.replace(/\/$/, '')}/${code}` : null;
}

/** The host of the public origin, or `''` when there is none or it is not a URL. */
export function bookingHost(origin: string): string {
	try {
		return origin ? new URL(origin).host : '';
	} catch {
		return '';
	}
}

/** What the server takes as a custom host: at least two labels of 1 to 63 letters, digits and inner hyphens. */
export const HOST_PATTERN =
	'([a-z0-9]([a-z0-9\\-]{0,61}[a-z0-9])?\\.)+[a-z0-9]([a-z0-9\\-]{0,61}[a-z0-9])?';
