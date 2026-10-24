import { describe, expect, it } from 'vitest';
import {
	boardColumns,
	formPrices,
	lineText,
	localDate,
	nextStep,
	orderLines,
	orderQuantities,
	orderTotal,
	priceGroups,
	type LaundryOrder,
	type LaundryPrice
} from './laundry';

const price = (
	id: string,
	itemCode: string,
	service: LaundryPrice['service'],
	amount: number
): LaundryPrice => ({
	id,
	itemCode,
	service,
	name: itemCode.toLowerCase(),
	price: amount,
	active: true,
	version: 1
});

const order = (
	id: string,
	status: LaundryOrder['status'],
	deliveredAt: string | null = null
): LaundryOrder => ({
	id,
	reservationRoomId: 'rr1',
	roomNumber: '101',
	guest: 'Ann Perera',
	status,
	note: null,
	lines: [],
	total: 0,
	currency: 'LKR',
	createdAt: '2026-10-24T08:00:00Z',
	statusAt: '2026-10-24T08:00:00Z',
	deliveredAt,
	version: 1
});

describe('priceGroups', () => {
	it('groups the prices by item code, each in service order', () => {
		const prices = [
			price('p1', 'TROUSERS', 'PRESS', 300),
			price('p2', 'SHIRT', 'EXPRESS', 900),
			price('p3', 'SHIRT', 'WASH', 400),
			price('p4', 'SHIRT', 'DRY_CLEAN', 700),
			price('p5', 'TROUSERS', 'WASH', 500)
		];

		expect(
			priceGroups(prices).map((group) => [group.itemCode, group.prices.map((p) => p.id)])
		).toEqual([
			['SHIRT', ['p3', 'p4', 'p2']],
			['TROUSERS', ['p5', 'p1']]
		]);
	});

	it('is empty for no prices', () => {
		expect(priceGroups([])).toEqual([]);
	});
});

describe('orderTotal', () => {
	const prices = [price('p1', 'SHIRT', 'WASH', 40000), price('p2', 'SHIRT', 'PRESS', 25050)];

	it('sums each price times its quantity', () => {
		expect(orderTotal(prices, { p1: 2, p2: 3 })).toBe(155150);
	});

	it('counts a blank quantity, or a price not listed, as nothing', () => {
		expect(orderTotal(prices, { p1: 1, p9: 5 })).toBe(40000);
		expect(orderTotal(prices, { p1: NaN as unknown as number })).toBe(0);
		expect(orderTotal(prices, {})).toBe(0);
	});
});

describe('orderLines', () => {
	it('sends each listed price with a quantity above 0, in list order', () => {
		const prices = [price('p1', 'A', 'WASH', 1), price('p2', 'A', 'PRESS', 1)];
		expect(orderLines(prices, { p2: 3, p1: 0, p9: 4 })).toEqual([{ price_id: 'p2', quantity: 3 }]);
	});
});

describe('orderQuantities', () => {
	it('sums a repeated price', () => {
		expect(
			orderQuantities([
				{ priceId: 'p1', quantity: 2 },
				{ priceId: 'p2', quantity: 1 },
				{ priceId: 'p1', quantity: 3 }
			])
		).toEqual({ p1: 5, p2: 1 });
	});
});

describe('formPrices', () => {
	it('offers the active list with the prices already on the order as they were taken', () => {
		const active = [price('p1', 'SHIRT', 'WASH', 900), price('p2', 'SUIT', 'PRESS', 300)];
		const taken = [
			{
				priceId: 'p1',
				itemCode: 'SHIRT',
				service: 'WASH' as const,
				name: 'old shirt',
				unitPrice: 500
			},
			{ priceId: 'p3', itemCode: 'TIE', service: 'PRESS' as const, name: 'tie', unitPrice: 200 }
		];
		const offered = formPrices(active, taken);
		expect(offered.map((entry) => [entry.id, entry.name, entry.price])).toEqual([
			['p1', 'old shirt', 500],
			['p2', 'suit', 300],
			['p3', 'tie', 200]
		]);
		expect(orderTotal(offered, { p1: 1, p2: 1, p3: 2 })).toBe(1_200);
	});

	it('is the active list for a new order', () => {
		const active = [price('p1', 'SHIRT', 'WASH', 900)];
		expect(formPrices(active, [])).toEqual(active);
	});
});

describe('nextStep', () => {
	it('labels the one step after each open status', () => {
		expect(nextStep('COLLECTED')).toEqual({ status: 'processing', label: 'Start' });
		expect(nextStep('PROCESSING')).toEqual({ status: 'ready', label: 'Ready' });
		expect(nextStep('READY')).toEqual({ status: 'delivered', label: 'Deliver' });
	});

	it('has none once delivered or cancelled', () => {
		expect(nextStep('DELIVERED')).toBeNull();
		expect(nextStep('CANCELLED')).toBeNull();
	});
});

describe('lineText', () => {
	it('names the item, the service and the quantity', () => {
		expect(lineText({ name: 'Shirt', service: 'DRY_CLEAN', quantity: 2 })).toBe(
			'Shirt (Dry clean) × 2'
		);
	});
});

describe('localDate', () => {
	it("reads the date in the property's time zone", () => {
		expect(localDate('2026-10-24T20:00:00Z', 'Asia/Colombo')).toBe('2026-10-25');
		expect(localDate('2026-10-24T20:00:00Z', 'UTC')).toBe('2026-10-24');
	});
});

describe('boardColumns', () => {
	it('puts open orders under their status and only today’s deliveries last, leaving off cancelled ones', () => {
		const orders = [
			order('o1', 'COLLECTED'),
			order('o2', 'PROCESSING'),
			order('o3', 'READY'),
			order('o4', 'DELIVERED', '2026-10-24T05:00:00Z'),
			order('o5', 'DELIVERED', '2026-10-23T05:00:00Z'),
			order('o6', 'CANCELLED'),
			order('o7', 'COLLECTED')
		];

		expect(
			boardColumns(orders, '2026-10-24', 'Asia/Colombo').map((column) => [
				column.label,
				column.orders.map((o) => o.id)
			])
		).toEqual([
			['Collected', ['o1', 'o7']],
			['Processing', ['o2']],
			['Ready', ['o3']],
			['Delivered today', ['o4']]
		]);
	});
});
