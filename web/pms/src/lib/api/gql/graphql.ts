/* eslint-disable */
/** Internal type. DO NOT USE DIRECTLY. */
type Exact<T extends { [key: string]: unknown }> = { [K in keyof T]: T[K] };
/** Internal type. DO NOT USE DIRECTLY. */
export type Incremental<T> = T | { [P in keyof T]?: P extends ' $fragmentName' | '__typename' ? T[P] : never };
import type { DocumentTypeDecoration } from '@graphql-typed-document-node/core';
export type BlockKind =
  /** Out of inventory: reduces availability. */
  | 'OUT_OF_ORDER'
  /** Still sellable; shown only. */
  | 'OUT_OF_SERVICE';

export type ChangeMode =
  | 'AMOUNT'
  | 'PERCENT';

export type MealPlan =
  | 'BB'
  | 'FB'
  | 'HB'
  | 'RO';

export type PlanKind =
  | 'CUSTOM'
  | 'DERIVED'
  | 'STANDARD';

export type PriceChangeMode =
  | 'AMOUNT'
  | 'PERCENT'
  | 'SET';

export type Residency =
  | 'NON_RESIDENT'
  | 'RESIDENT';

export type Segment =
  | 'FIT_F'
  | 'FIT_L'
  | 'IBE'
  | 'OTA'
  | 'TA';

export type ViolationKind =
  | 'CLOSED'
  | 'CLOSED_TO_ARRIVAL'
  | 'CLOSED_TO_DEPARTURE'
  | 'INACTIVE'
  | 'INVALID_STAY'
  | 'MAX_STAY'
  | 'MEAL_PLAN_NOT_ALLOWED'
  | 'MIN_STAY'
  | 'NO_MEAL_SUPPLEMENT'
  | 'NO_PRICE'
  | 'OCCUPANCY'
  | 'RESIDENCY'
  | 'ROOM_TYPE_NOT_SOLD';

export type InventoryQueryVariables = Exact<{
  propertyId: string;
  from: string;
  to: string;
}>;


export type InventoryQuery = { inventory: Array<{ date: string, roomTypeId: string, physical: number, sold: number, outOfOrder: number, available: number }>, blocks: Array<{ id: string, roomId: string, from: string, to: string, kind: BlockKind, reasonId: string, note: string, version: number }> };

export type PropertiesQueryVariables = Exact<{ [key: string]: never; }>;


export type PropertiesQuery = { properties: Array<{ id: string, code: string, name: string, timezone: string, baseCurrency: string, businessDate: string, checkInTime: string, checkOutTime: string, version: number }> };

export type RatePlansQueryVariables = Exact<{
  propertyId: string;
}>;


export type RatePlansQuery = { ratePlans: Array<{ id: string, code: string, name: string, kind: PlanKind, segment: Segment, residency: Residency | null, currency: string, parentId: string | null, depth: number, deriveMode: ChangeMode | null, deriveValue: number | null, roundingStep: number, extraAdultAmount: number, inheritRestrictions: boolean, allowedMealPlans: Array<MealPlan>, cancellationPolicyId: string | null, roomTypeIds: Array<string>, active: boolean, version: number }>, mealSupplements: Array<{ id: string, mealPlan: MealPlan, currency: string, adultAmount: number, childAmount: number, from: string, to: string | null, version: number }>, cancellationPolicies: Array<{ id: string, name: string, version: number }> };

export type RateGridQueryVariables = Exact<{
  propertyId: string;
  ratePlanId: string;
  from: string;
  to: string;
}>;


export type RateGridQuery = { rateGrid: { prices: Array<{ roomTypeId: string, date: string, occupancy: number, amount: number }>, restrictions: Array<{ roomTypeId: string, date: string, closed: boolean, minStay: number | null, maxStay: number | null, closedToArrival: boolean, closedToDeparture: boolean }> } };

export type BulkPreviewQueryVariables = Exact<{
  propertyId: string;
  ratePlanId: string;
  from: string;
  to: string;
  weekdays: Array<number> | number;
  roomTypeIds: Array<string> | string;
  mode: PriceChangeMode;
  value: number;
}>;


export type BulkPreviewQuery = { bulkChangePreview: { total: number, cells: Array<{ roomTypeId: string, date: string, occupancy: number, before: number | null, after: number }> } };

export type QuoteQueryVariables = Exact<{
  propertyId: string;
  roomTypeId: string;
  ratePlanId: string;
  mealPlan: MealPlan;
  checkIn: string;
  checkOut: string;
  adults: number;
  children: number;
  residency: Residency;
}>;


export type QuoteQuery = { quote: { total: number, currency: string, restrictionsOk: boolean, nights: Array<{ date: string, room: number, meal: number }>, violations: Array<{ kind: ViolationKind, date: string | null, message: string }> } };

export type RoomTypesQueryVariables = Exact<{
  propertyId: string;
}>;


export type RoomTypesQuery = { roomTypes: Array<{ id: string, code: string, name: string, baseOccupancy: number, maxAdults: number, maxChildren: number, maxOccupancy: number, amenities: Array<string>, sortOrder: number, active: boolean, version: number }> };

export type RoomsQueryVariables = Exact<{
  propertyId: string;
}>;


export type RoomsQuery = { rooms: Array<{ id: string, roomTypeId: string, number: string, floor: string | null, sectionId: string | null, active: boolean, sortOrder: number, version: number }>, sections: Array<{ id: string, name: string, version: number }>, blockReasons: Array<{ id: string, code: string, label: string, defaultKind: BlockKind, active: boolean }> };

export class TypedDocumentString<TResult, TVariables>
  extends String
  implements DocumentTypeDecoration<TResult, TVariables>
{
  __apiType?: NonNullable<DocumentTypeDecoration<TResult, TVariables>['__apiType']>;
  private value: string;
  public __meta__?: Record<string, any> | undefined;

  constructor(value: string, __meta__?: Record<string, any> | undefined) {
    super(value);
    this.value = value;
    this.__meta__ = __meta__;
  }

  override toString(): string & DocumentTypeDecoration<TResult, TVariables> {
    return this.value;
  }
}

export const InventoryDocument = new TypedDocumentString(`
    query Inventory($propertyId: UUID!, $from: Date!, $to: Date!) {
  inventory(propertyId: $propertyId, from: $from, to: $to) {
    date
    roomTypeId
    physical
    sold
    outOfOrder
    available
  }
  blocks(propertyId: $propertyId, from: $from, to: $to) {
    id
    roomId
    from
    to
    kind
    reasonId
    note
    version
  }
}
    `) as unknown as TypedDocumentString<InventoryQuery, InventoryQueryVariables>;
export const PropertiesDocument = new TypedDocumentString(`
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
    `) as unknown as TypedDocumentString<PropertiesQuery, PropertiesQueryVariables>;
export const RatePlansDocument = new TypedDocumentString(`
    query RatePlans($propertyId: UUID!) {
  ratePlans(propertyId: $propertyId) {
    id
    code
    name
    kind
    segment
    residency
    currency
    parentId
    depth
    deriveMode
    deriveValue
    roundingStep
    extraAdultAmount
    inheritRestrictions
    allowedMealPlans
    cancellationPolicyId
    roomTypeIds
    active
    version
  }
  mealSupplements(propertyId: $propertyId) {
    id
    mealPlan
    currency
    adultAmount
    childAmount
    from
    to
    version
  }
  cancellationPolicies(propertyId: $propertyId) {
    id
    name
    version
  }
}
    `) as unknown as TypedDocumentString<RatePlansQuery, RatePlansQueryVariables>;
export const RateGridDocument = new TypedDocumentString(`
    query RateGrid($propertyId: UUID!, $ratePlanId: UUID!, $from: Date!, $to: Date!) {
  rateGrid(propertyId: $propertyId, ratePlanId: $ratePlanId, from: $from, to: $to) {
    prices {
      roomTypeId
      date
      occupancy
      amount
    }
    restrictions {
      roomTypeId
      date
      closed
      minStay
      maxStay
      closedToArrival
      closedToDeparture
    }
  }
}
    `) as unknown as TypedDocumentString<RateGridQuery, RateGridQueryVariables>;
export const BulkPreviewDocument = new TypedDocumentString(`
    query BulkPreview($propertyId: UUID!, $ratePlanId: UUID!, $from: Date!, $to: Date!, $weekdays: [Int!]!, $roomTypeIds: [UUID!]!, $mode: PriceChangeMode!, $value: Int!) {
  bulkChangePreview(
    propertyId: $propertyId
    ratePlanId: $ratePlanId
    from: $from
    to: $to
    weekdays: $weekdays
    roomTypeIds: $roomTypeIds
    mode: $mode
    value: $value
  ) {
    total
    cells {
      roomTypeId
      date
      occupancy
      before
      after
    }
  }
}
    `) as unknown as TypedDocumentString<BulkPreviewQuery, BulkPreviewQueryVariables>;
export const QuoteDocument = new TypedDocumentString(`
    query Quote($propertyId: UUID!, $roomTypeId: UUID!, $ratePlanId: UUID!, $mealPlan: MealPlan!, $checkIn: Date!, $checkOut: Date!, $adults: Int!, $children: Int!, $residency: Residency!) {
  quote(
    propertyId: $propertyId
    roomTypeId: $roomTypeId
    ratePlanId: $ratePlanId
    mealPlan: $mealPlan
    checkIn: $checkIn
    checkOut: $checkOut
    adults: $adults
    children: $children
    residency: $residency
  ) {
    nights {
      date
      room
      meal
    }
    total
    currency
    restrictionsOk
    violations {
      kind
      date
      message
    }
  }
}
    `) as unknown as TypedDocumentString<QuoteQuery, QuoteQueryVariables>;
export const RoomTypesDocument = new TypedDocumentString(`
    query RoomTypes($propertyId: UUID!) {
  roomTypes(propertyId: $propertyId) {
    id
    code
    name
    baseOccupancy
    maxAdults
    maxChildren
    maxOccupancy
    amenities
    sortOrder
    active
    version
  }
}
    `) as unknown as TypedDocumentString<RoomTypesQuery, RoomTypesQueryVariables>;
export const RoomsDocument = new TypedDocumentString(`
    query Rooms($propertyId: UUID!) {
  rooms(propertyId: $propertyId) {
    id
    roomTypeId
    number
    floor
    sectionId
    active
    sortOrder
    version
  }
  sections(propertyId: $propertyId) {
    id
    name
    version
  }
  blockReasons(propertyId: $propertyId) {
    id
    code
    label
    defaultKind
    active
  }
}
    `) as unknown as TypedDocumentString<RoomsQuery, RoomsQueryVariables>;