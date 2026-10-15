import type { components } from './api/openapi';
import { graphql } from './api/gql';
import type { GuestDocumentsQuery } from './api/gql/graphql';
import { query } from './api/graphql';
import { rest, unwrap } from './api/rest';
import { mediaKey } from './media';
import type { DocumentKind } from './upload';

export const GuestDocumentsDocument = graphql(`
	query GuestDocuments($propertyId: UUID!, $guestId: UUID!) {
		guestDocuments(propertyId: $propertyId, guestId: $guestId) {
			id
			kind
			status
			mime
			uploadedAt
			uploadedBy
		}
	}
`);

export type GuestDocument = GuestDocumentsQuery['guestDocuments'][number];
export type LinkSize = components['schemas']['LinkSize'];
export type SignedLink = components['schemas']['SignedLink'];

/**
 * A guest's documents at the property, under `media:<property>` so that every upload, finished
 * processing and deletion refetches them. Only the list is cached, never a link to a file.
 */
export function guestDocumentsKey(propertyId: string, guestId: string) {
	return [...mediaKey(propertyId), 'guest-documents', guestId] as const;
}

/** The guest's documents uploaded at the property, newest first (at most 50). */
export async function fetchGuestDocuments(
	propertyId: string,
	guestId: string,
	signal?: AbortSignal
): Promise<GuestDocument[]> {
	return (await query(GuestDocumentsDocument, { propertyId, guestId }, signal)).guestDocuments;
}

/** The kinds, in the order the upload's select offers them. */
export const DOCUMENT_KINDS: readonly DocumentKind[] = [
	'passport',
	'nic',
	'driving_licence',
	'other'
];

const KIND_LABELS: Record<DocumentKind, string> = {
	passport: 'Passport',
	nic: 'NIC',
	driving_licence: 'Driving licence',
	other: 'Other'
};

/** A kind as the UI names it; takes the API's lower case or GraphQL's upper case. */
export function documentKindLabel(kind: DocumentKind | GuestDocument['kind']): string {
	return KIND_LABELS[kind.toLowerCase() as DocumentKind];
}

export const PDF = 'application/pdf';

/** The file a view opens: a PDF as uploaded, an image at its `md` size (its original is JPEG XL). */
export function viewSize(mime: string | null | undefined): LinkSize {
	return mime === PDF ? 'original' : 'md';
}

/** Who uploaded a document, as far as the list can tell: `uploadedBy` is only a user's id. */
export function uploaderLabel(uploadedBy: string | null | undefined, me: string): string {
	if (!uploadedBy) return 'A deleted user';
	return uploadedBy === me ? 'You' : 'Another user';
}

/**
 * A fresh link to the document's file, good for 5 minutes. Each call is audited on the server, so it
 * is made once per view and its answer is never cached.
 */
export async function documentLink(
	propertyId: string,
	documentId: string,
	size: LinkSize
): Promise<SignedLink> {
	return unwrap(
		await rest.GET('/api/v1/properties/{property}/guest-documents/{document}/link', {
			params: { path: { property: propertyId, document: documentId }, query: { size } }
		})
	);
}

export async function deleteGuestDocument(propertyId: string, documentId: string) {
	unwrap(
		await rest.DELETE('/api/v1/properties/{property}/guest-documents/{document}', {
			params: { path: { property: propertyId, document: documentId } }
		})
	);
}
