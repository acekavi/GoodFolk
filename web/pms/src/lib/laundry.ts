import { graphql } from './api/gql';
import type { LaundryOrdersQuery, LaundryPricesQuery } from './api/gql/graphql';
import { query } from './api/graphql';
import type { components } from './api/openapi';

/** The guest laundry price list: active prices only unless `includeInactive` (the Laundry prices page). */
export const LaundryPricesDocument = graphql(`
	query LaundryPrices($propertyId: UUID!, $includeInactive: Boolean!) {
		laundryPrices(propertyId: $propertyId, includeInactive: $includeInactive) {
			id
			itemCode
			service
			name
			price
			active
			version
		}
	}
`);

/** The Guest laundry board: every order, open ones first. */
export const LaundryOrdersDocument = graphql(`
	query LaundryOrders($propertyId: UUID!) {
		laundryOrders(propertyId: $propertyId) {
			id
			reservationRoomId
			roomNumber
			guest
			status
			note
			lines {
				position
				priceId
				itemCode
				service
				name
				unitPrice
				quantity
			}
			total
			currency
			createdAt
			statusAt
			deliveredAt
			version
		}
	}
`);

/** One stay's orders, for its room card in the reservation modal. */
export const LaundryOrdersForStayDocument = graphql(`
	query LaundryOrdersForStay($propertyId: UUID!, $reservationRoomId: UUID!) {
		laundryOrdersForStay(propertyId: $propertyId, reservationRoomId: $reservationRoomId) {
			id
			reservationRoomId
			roomNumber
			guest
			status
			note
			lines {
				position
				priceId
				itemCode
				service
				name
				unitPrice
				quantity
			}
			total
			currency
			createdAt
			statusAt
			deliveredAt
			version
		}
	}
`);

export type LaundryPrice = LaundryPricesQuery['laundryPrices'][number];
export type LaundryOrder = LaundryOrdersQuery['laundryOrders'][number];
export type LaundryService = LaundryPrice['service'];
export type LaundryOrderStatus = LaundryOrder['status'];
/** A status as the REST API takes it. */
export type OrderStatusValue = components['schemas']['OrderStatus'];
/** A service as the REST API takes it. */
export type ServiceValue = components['schemas']['Service'];

/**
 * Every laundry query is under the server's `laundry:<property>` event key, so the event refetches them all
 * (`events.ts`). `laundryKey(propertyId)` alone is that prefix.
 */
export function laundryKey(propertyId: string, ...rest: string[]) {
	return [`laundry:${propertyId}`, ...rest] as const;
}

export async function fetchLaundryPrices(
	propertyId: string,
	includeInactive: boolean,
	signal?: AbortSignal
) {
	return (await query(LaundryPricesDocument, { propertyId, includeInactive }, signal))
		.laundryPrices;
}

export async function fetchLaundryOrders(propertyId: string, signal?: AbortSignal) {
	return (await query(LaundryOrdersDocument, { propertyId }, signal)).laundryOrders;
}

export async function fetchStayLaundryOrders(
	propertyId: string,
	reservationRoomId: string,
	signal?: AbortSignal
) {
	return (await query(LaundryOrdersForStayDocument, { propertyId, reservationRoomId }, signal))
		.laundryOrdersForStay;
}

const SERVICES: readonly LaundryService[] = ['WASH', 'PRESS', 'DRY_CLEAN', 'EXPRESS'];

const SERVICE_LABELS: Record<LaundryService, string> = {
	WASH: 'Wash',
	PRESS: 'Press',
	DRY_CLEAN: 'Dry clean',
	EXPRESS: 'Express'
};

export function serviceLabel(service: LaundryService): string {
	return SERVICE_LABELS[service];
}

/** The services a new price can be for, as the REST API takes them, with their labels. */
export const SERVICE_OPTIONS: readonly { value: ServiceValue; label: string }[] = SERVICES.map(
	(service) => ({ value: service.toLowerCase() as ServiceValue, label: SERVICE_LABELS[service] })
);

export interface PriceGroup {
	itemCode: string;
	prices: LaundryPrice[];
}

/** The price list by item code, each item's services in the order wash, press, dry clean, express. */
export function priceGroups(prices: readonly LaundryPrice[]): PriceGroup[] {
	const groups = new Map<string, LaundryPrice[]>();
	for (const price of prices) {
		const group = groups.get(price.itemCode);
		if (group) group.push(price);
		else groups.set(price.itemCode, [price]);
	}
	return [...groups.entries()]
		.sort(([a], [b]) => a.localeCompare(b))
		.map(([itemCode, group]) => ({
			itemCode,
			prices: group.sort((a, b) => SERVICES.indexOf(a.service) - SERVICES.indexOf(b.service))
		}));
}

/** What the form's quantities (by price id) cost at the listed prices, net of tax; a price not listed counts 0. */
export function orderTotal(
	prices: readonly Pick<LaundryPrice, 'id' | 'price'>[],
	quantities: Readonly<Record<string, number>>
): number {
	return prices.reduce((sum, price) => sum + price.price * (quantities[price.id] || 0), 0);
}

/** The lines a request sends, `{price_id, quantity}` for each listed price with a quantity above 0, in list order. */
export function orderLines(
	prices: readonly Pick<LaundryPrice, 'id'>[],
	quantities: Readonly<Record<string, number>>
): { price_id: string; quantity: number }[] {
	return prices
		.filter((price) => (quantities[price.id] || 0) > 0)
		.map((price) => ({ price_id: price.id, quantity: quantities[price.id] }));
}

/** An order's quantities by price, a repeated price summed: the form's starting point for an edit. */
export function orderQuantities(
	lines: readonly Pick<LaundryOrder['lines'][number], 'priceId' | 'quantity'>[]
): Record<string, number> {
	const quantities: Record<string, number> = {};
	for (const line of lines)
		quantities[line.priceId] = (quantities[line.priceId] ?? 0) + line.quantity;
	return quantities;
}

/** The step after `status`, with its button label; `null` once delivered or cancelled. */
export function nextStep(
	status: LaundryOrderStatus
): { status: OrderStatusValue; label: string } | null {
	switch (status) {
		case 'COLLECTED':
			return { status: 'processing', label: 'Start' };
		case 'PROCESSING':
			return { status: 'ready', label: 'Ready' };
		case 'READY':
			return { status: 'delivered', label: 'Deliver' };
		default:
			return null;
	}
}

const STATUS_LABELS: Record<LaundryOrderStatus, string> = {
	COLLECTED: 'Collected',
	PROCESSING: 'Processing',
	READY: 'Ready',
	DELIVERED: 'Delivered',
	CANCELLED: 'Cancelled'
};

export function orderStatusLabel(status: LaundryOrderStatus): string {
	return STATUS_LABELS[status];
}

/** An order line in words: `Shirt (Wash) × 2`. */
export function lineText(
	line: Pick<LaundryOrder['lines'][number], 'name' | 'service' | 'quantity'>
) {
	return `${line.name} (${serviceLabel(line.service)}) × ${line.quantity}`;
}

/** The calendar date of `at` in `timeZone`, as `YYYY-MM-DD`. */
export function localDate(at: Date | string, timeZone: string): string {
	return new Intl.DateTimeFormat('en-CA', {
		timeZone,
		year: 'numeric',
		month: '2-digit',
		day: '2-digit'
	}).format(new Date(at));
}

export interface BoardColumn {
	id: 'collected' | 'processing' | 'ready' | 'delivered';
	label: string;
	orders: LaundryOrder[];
}

/**
 * The board's columns: the open orders by status, and those delivered on `today` (a date in `timeZone`).
 * Cancelled orders and earlier deliveries are left off.
 */
export function boardColumns(
	orders: readonly LaundryOrder[],
	today: string,
	timeZone: string
): BoardColumn[] {
	const by = (status: LaundryOrderStatus) => orders.filter((order) => order.status === status);
	return [
		{ id: 'collected', label: 'Collected', orders: by('COLLECTED') },
		{ id: 'processing', label: 'Processing', orders: by('PROCESSING') },
		{ id: 'ready', label: 'Ready', orders: by('READY') },
		{
			id: 'delivered',
			label: 'Delivered today',
			orders: by('DELIVERED').filter(
				(order) => order.deliveredAt && localDate(order.deliveredAt, timeZone) === today
			)
		}
	];
}
