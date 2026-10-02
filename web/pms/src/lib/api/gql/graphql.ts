/* eslint-disable */
/** Internal type. DO NOT USE DIRECTLY. */
type Exact<T extends { [key: string]: unknown }> = { [K in keyof T]: T[K] };
/** Internal type. DO NOT USE DIRECTLY. */
export type Incremental<T> = T | { [P in keyof T]?: P extends ' $fragmentName' | '__typename' ? T[P] : never };
import type { DocumentTypeDecoration } from '@graphql-typed-document-node/core';
export type AccountKind =
  | 'COMPANY'
  | 'TRAVEL_AGENT';

export type BlockKind =
  /** Out of inventory: reduces availability. */
  | 'OUT_OF_ORDER'
  /** Still sellable; shown only. */
  | 'OUT_OF_SERVICE';

export type ChangeMode =
  | 'AMOUNT'
  | 'PERCENT';

export type IdDocType =
  | 'DRIVING_LICENCE'
  | 'NIC'
  | 'OTHER'
  | 'PASSPORT';

export type MealPlan =
  | 'BB'
  | 'FB'
  | 'HB'
  | 'RO';

/** Why a stay has no room. */
export type NeedsRoomReason =
  | 'NO_SINGLE_ROOM'
  | 'OVERBOOKED';

export type PenaltyKind =
  | 'AMOUNT'
  | 'NIGHTS'
  | 'PERCENT';

export type PlanKind =
  | 'CUSTOM'
  | 'DERIVED'
  | 'STANDARD';

export type PriceChangeMode =
  | 'AMOUNT'
  | 'PERCENT'
  | 'SET';

/**
 * Which reservation rooms to list. Left out, a field does not filter; an empty `statuses` or `sources`
 * matches nothing.
 */
export type ReservationFilter = {
  arrivalFrom?: string | null | undefined;
  /** Inclusive. */
  arrivalTo?: string | null | undefined;
  sources?: Array<Source> | null | undefined;
  statuses?: Array<RoomStatus> | null | undefined;
  /** The start of a confirmation number, in any case, or a guest's name, typos included. */
  text?: string | null | undefined;
};

export type ReservationSort = {
  direction?: SortDirection;
  field: ReservationSortField;
};

/** What the reservations list is sorted by; ties go by the room's id. */
export type ReservationSortField =
  | 'ARRIVAL'
  | 'CONFIRMATION'
  | 'CREATED'
  | 'GUEST';

export type Residency =
  | 'NON_RESIDENT'
  | 'RESIDENT';

export type RoomStatus =
  | 'CANCELLED'
  | 'CHECKED_IN'
  | 'CHECKED_OUT'
  | 'CONFIRMED'
  | 'NO_SHOW'
  | 'TENTATIVE';

export type Segment =
  | 'FIT_F'
  | 'FIT_L'
  | 'IBE'
  | 'OTA'
  | 'TA';

export type SortDirection =
  | 'ASC'
  | 'DESC';

export type Source =
  | 'CHANNEL'
  | 'EMAIL'
  | 'FRONT_DESK'
  | 'IBE'
  | 'PHONE';

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

export type AccountsQueryVariables = Exact<{
  propertyId: string;
  search?: string | null | undefined;
  includeInactive?: boolean | null | undefined;
}>;


export type AccountsQuery = { accounts: Array<{ id: string, kind: AccountKind, name: string, creditLimit: number | null, currency: string, active: boolean, version: number, contact: { email: string | null, phone: string | null, address: string | null, contactName: string | null } }> };

export type InventoryQueryVariables = Exact<{
  propertyId: string;
  from: string;
  to: string;
}>;


export type InventoryQuery = { inventory: Array<{ date: string, roomTypeId: string, physical: number, sold: number, outOfOrder: number, available: number, sellable: number }>, blocks: Array<{ id: string, roomId: string, from: string, to: string, kind: BlockKind, reasonId: string, note: string, version: number }> };

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

export type AvailabilityQueryVariables = Exact<{
  propertyId: string;
  checkIn: string;
  checkOut: string;
  adults: number;
  children: number;
  residency: Residency;
}>;


export type AvailabilityQuery = { availability: Array<{ roomTypeId: string, code: string, name: string, free: number, offers: Array<{ ratePlanId: string, ratePlanCode: string, mealPlan: MealPlan, total: number, currency: string, restrictionsOk: boolean, violations: Array<{ kind: ViolationKind, message: string }>, nights: Array<{ date: string, room: number, meal: number }> }> }> };

export type ReservationListQueryVariables = Exact<{
  p: string;
  filter?: ReservationFilter | null | undefined;
  sort?: ReservationSort | null | undefined;
  first?: number | null | undefined;
  after?: string | null | undefined;
  withCount: boolean;
}>;


export type ReservationListQuery = { reservations: { totalCount?: number, nodes: Array<{ id: string, reservationId: string, confirmationNo: string, guestName: string, arrival: string, departure: string, nights: number, roomTypeCode: string, roomNumber: string | null, status: RoomStatus, source: Source, total: number, currency: string, version: number, accountName: string | null }>, pageInfo: { endCursor: string | null, hasNextPage: boolean } } };

export type ReservationQueryVariables = Exact<{
  p: string;
  id: string;
}>;


export type ReservationQuery = { reservation: { id: string, confirmationNo: string, status: RoomStatus, source: Source, notes: string, createdAt: string, version: number, booker: { id: string, firstName: string, lastName: string, email: string | null, phone: string | null, country: string | null, residency: Residency, idDocType: IdDocType | null, idDocMasked: string | null, notes: string, version: number }, account: { id: string, name: string, kind: AccountKind } | null, totals: Array<{ currency: string, amount: number }>, rooms: Array<{ id: string, version: number, status: RoomStatus, checkIn: string, checkOut: string, adults: number, children: number, mealPlan: MealPlan, total: number, currency: string, cancellationPenalty: number | null, cancelledAt: string | null, recordedPenalty: number | null, checkedInAt: string | null, checkedInBusinessDate: string | null, checkedOutAt: string | null, canCheckIn: boolean, canUndoCheckIn: boolean, canCheckOut: boolean, roomType: { id: string, code: string, name: string }, room: { id: string, number: string } | null, ratePlan: { id: string, code: string }, primaryGuest: { id: string, firstName: string, lastName: string, residency: Residency, idDocType: IdDocType | null, idDocMasked: string | null }, occupants: Array<{ id: string, firstName: string, lastName: string, residency: Residency, idDocType: IdDocType | null, idDocMasked: string | null }>, nights: Array<{ date: string, room: number, meal: number }>, cancellationTerms: { rules: Array<{ daysBeforeArrival: number, penalty: { kind: PenaltyKind, value: number } }>, noShow: { kind: PenaltyKind, value: number } } | null }>, history: Array<{ action: string, at: string, actorName: string | null, data: unknown }> } };

export type GuestsQueryVariables = Exact<{
  propertyId: string;
  search?: string | null | undefined;
  first?: number | null | undefined;
}>;


export type GuestsQuery = { guests: Array<{ id: string, firstName: string, lastName: string, email: string | null, phone: string | null, country: string | null, residency: Residency, idDocType: IdDocType | null, idDocMasked: string | null, notes: string, version: number }> };

export type FreeRoomsQueryVariables = Exact<{
  propertyId: string;
  roomTypeId: string;
  checkIn: string;
  checkOut: string;
}>;


export type FreeRoomsQuery = { freeRooms: Array<{ id: string, number: string, section: string | null }> };

export type RoomTypesQueryVariables = Exact<{
  propertyId: string;
}>;


export type RoomTypesQuery = { roomTypes: Array<{ id: string, code: string, name: string, baseOccupancy: number, maxAdults: number, maxChildren: number, maxOccupancy: number, overbooking: number, amenities: Array<string>, sortOrder: number, active: boolean, version: number }> };

export type RoomsQueryVariables = Exact<{
  propertyId: string;
}>;


export type RoomsQuery = { rooms: Array<{ id: string, roomTypeId: string, number: string, floor: string | null, sectionId: string | null, active: boolean, sortOrder: number, version: number }>, sections: Array<{ id: string, name: string, version: number }>, blockReasons: Array<{ id: string, code: string, label: string, defaultKind: BlockKind, active: boolean }> };

export type TapeWindowQueryVariables = Exact<{
  property: string;
  rooms: Array<string> | string;
  from: string;
  to: string;
}>;


export type TapeWindowQuery = { tapeWindow: { stays: Array<{ id: string, reservationId: string, roomId: string, roomTypeId: string, start: string, end: string, status: RoomStatus, guestName: string, accountName: string | null, version: number }>, blocks: Array<{ id: string, roomId: string, start: string, end: string, reason: string }> } };

export type UnassignedStaysQueryVariables = Exact<{
  property: string;
  from: string;
  to: string;
}>;


export type UnassignedStaysQuery = { unassignedStays: Array<{ id: string, reservationId: string, roomTypeId: string, start: string, end: string, status: RoomStatus, guestName: string, reason: NeedsRoomReason, version: number }> };

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

export const AccountsDocument = new TypedDocumentString(`
    query Accounts($propertyId: UUID!, $search: String, $includeInactive: Boolean = false) {
  accounts(
    propertyId: $propertyId
    search: $search
    includeInactive: $includeInactive
  ) {
    id
    kind
    name
    contact {
      email
      phone
      address
      contactName
    }
    creditLimit
    currency
    active
    version
  }
}
    `, {"hash":"sha256:0991236e571c94b801691aefc5b9a85c0c1372010a662f1ee98f155b11a7e2d8"}) as unknown as TypedDocumentString<AccountsQuery, AccountsQueryVariables>;
export const InventoryDocument = new TypedDocumentString(`
    query Inventory($propertyId: UUID!, $from: Date!, $to: Date!) {
  inventory(propertyId: $propertyId, from: $from, to: $to) {
    date
    roomTypeId
    physical
    sold
    outOfOrder
    available
    sellable
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
    `, {"hash":"sha256:17b30b76d3db9cfcb676c32ecfbe9846f2b10eee551be01de39ff387d20b4230"}) as unknown as TypedDocumentString<InventoryQuery, InventoryQueryVariables>;
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
    `, {"hash":"sha256:3cf9cbae0dcdc6b8cd0bd8a8e7df7dd0055510eb42b0458a0d6f1781feaf207b"}) as unknown as TypedDocumentString<PropertiesQuery, PropertiesQueryVariables>;
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
    `, {"hash":"sha256:1a16d95a5cd35bfc8bce15bd9d7c39071bfb080de1f7d8fbe2d90e3e52f532f0"}) as unknown as TypedDocumentString<RatePlansQuery, RatePlansQueryVariables>;
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
    `, {"hash":"sha256:a6766b4cdd715b62a3607b75d8fc70b7389d1d628def196d09ded97afcf7a2f0"}) as unknown as TypedDocumentString<RateGridQuery, RateGridQueryVariables>;
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
    `, {"hash":"sha256:8b4b36a381d7716668de9071a80419914b3945ff7cd77e15b40013477160f8da"}) as unknown as TypedDocumentString<BulkPreviewQuery, BulkPreviewQueryVariables>;
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
    `, {"hash":"sha256:4382aebfc395da7beaab46f722853f59e396b51fb4bf3f70a29880c54ff33fc0"}) as unknown as TypedDocumentString<QuoteQuery, QuoteQueryVariables>;
export const AvailabilityDocument = new TypedDocumentString(`
    query Availability($propertyId: UUID!, $checkIn: Date!, $checkOut: Date!, $adults: Int!, $children: Int!, $residency: Residency!) {
  availability(
    propertyId: $propertyId
    checkIn: $checkIn
    checkOut: $checkOut
    adults: $adults
    children: $children
    residency: $residency
  ) {
    roomTypeId
    code
    name
    free
    offers {
      ratePlanId
      ratePlanCode
      mealPlan
      total
      currency
      restrictionsOk
      violations {
        kind
        message
      }
      nights {
        date
        room
        meal
      }
    }
  }
}
    `, {"hash":"sha256:e8238b3d4b70996cc32f66f39de90746856e683d616946a40947d37bff9f5856"}) as unknown as TypedDocumentString<AvailabilityQuery, AvailabilityQueryVariables>;
export const ReservationListDocument = new TypedDocumentString(`
    query ReservationList($p: UUID!, $filter: ReservationFilter, $sort: ReservationSort, $first: Int, $after: String, $withCount: Boolean!) {
  reservations(
    propertyId: $p
    filter: $filter
    sort: $sort
    first: $first
    after: $after
  ) {
    nodes {
      id
      reservationId
      confirmationNo
      guestName
      arrival
      departure
      nights
      roomTypeCode
      roomNumber
      status
      source
      total
      currency
      version
      accountName
    }
    pageInfo {
      endCursor
      hasNextPage
    }
    totalCount @include(if: $withCount)
  }
}
    `, {"hash":"sha256:806941be1c1e89fbe3e1e55b9c67d63994c7ad8e12ec62ef1d9cfd8c4a44062a"}) as unknown as TypedDocumentString<ReservationListQuery, ReservationListQueryVariables>;
export const ReservationDocument = new TypedDocumentString(`
    query Reservation($p: UUID!, $id: UUID!) {
  reservation(propertyId: $p, id: $id) {
    id
    confirmationNo
    status
    source
    notes
    createdAt
    version
    booker {
      id
      firstName
      lastName
      email
      phone
      country
      residency
      idDocType
      idDocMasked
      notes
      version
    }
    account {
      id
      name
      kind
    }
    totals {
      currency
      amount
    }
    rooms {
      id
      version
      status
      checkIn
      checkOut
      adults
      children
      mealPlan
      total
      currency
      roomType {
        id
        code
        name
      }
      room {
        id
        number
      }
      ratePlan {
        id
        code
      }
      primaryGuest {
        id
        firstName
        lastName
        residency
        idDocType
        idDocMasked
      }
      occupants {
        id
        firstName
        lastName
        residency
        idDocType
        idDocMasked
      }
      nights {
        date
        room
        meal
      }
      cancellationTerms {
        rules {
          daysBeforeArrival
          penalty {
            kind
            value
          }
        }
        noShow {
          kind
          value
        }
      }
      cancellationPenalty
      cancelledAt
      recordedPenalty
      checkedInAt
      checkedInBusinessDate
      checkedOutAt
      canCheckIn
      canUndoCheckIn
      canCheckOut
    }
    history {
      action
      at
      actorName
      data
    }
  }
}
    `, {"hash":"sha256:2a513b095e6041e8d166928f61842645c9133d3257011e38613d61a37d4617d7"}) as unknown as TypedDocumentString<ReservationQuery, ReservationQueryVariables>;
export const GuestsDocument = new TypedDocumentString(`
    query Guests($propertyId: UUID!, $search: String, $first: Int) {
  guests(propertyId: $propertyId, search: $search, first: $first) {
    id
    firstName
    lastName
    email
    phone
    country
    residency
    idDocType
    idDocMasked
    notes
    version
  }
}
    `, {"hash":"sha256:246bfc3a00a93bff24de252cc9378d327ab3434a81c8eaa052675d9be7c57eed"}) as unknown as TypedDocumentString<GuestsQuery, GuestsQueryVariables>;
export const FreeRoomsDocument = new TypedDocumentString(`
    query FreeRooms($propertyId: UUID!, $roomTypeId: UUID!, $checkIn: Date!, $checkOut: Date!) {
  freeRooms(
    propertyId: $propertyId
    roomTypeId: $roomTypeId
    checkIn: $checkIn
    checkOut: $checkOut
  ) {
    id
    number
    section
  }
}
    `, {"hash":"sha256:f3d95d4297603facba0f9de166d3b1f2c1f03ff5a5ebd7ce69bdc70962c532d5"}) as unknown as TypedDocumentString<FreeRoomsQuery, FreeRoomsQueryVariables>;
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
    overbooking
    amenities
    sortOrder
    active
    version
  }
}
    `, {"hash":"sha256:e944b6bd50087c8b30c3842d1992880957eaa4f49c2d0fec0f60837b8b04b55d"}) as unknown as TypedDocumentString<RoomTypesQuery, RoomTypesQueryVariables>;
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
    `, {"hash":"sha256:c0abb0a62c7991a6c1c5d92b40c5daccafac9290699b81d370c6c2f0a5ce0ca6"}) as unknown as TypedDocumentString<RoomsQuery, RoomsQueryVariables>;
export const TapeWindowDocument = new TypedDocumentString(`
    query TapeWindow($property: UUID!, $rooms: [UUID!]!, $from: Date!, $to: Date!) {
  tapeWindow(propertyId: $property, roomIds: $rooms, from: $from, to: $to) {
    stays {
      id
      reservationId
      roomId
      roomTypeId
      start
      end
      status
      guestName
      accountName
      version
    }
    blocks {
      id
      roomId
      start
      end
      reason
    }
  }
}
    `, {"hash":"sha256:bc62e37b267ab6f2a849d6e4fc693f8e464217aa9a9a03dc9f54a095d509982c"}) as unknown as TypedDocumentString<TapeWindowQuery, TapeWindowQueryVariables>;
export const UnassignedStaysDocument = new TypedDocumentString(`
    query UnassignedStays($property: UUID!, $from: Date!, $to: Date!) {
  unassignedStays(propertyId: $property, from: $from, to: $to) {
    id
    reservationId
    roomTypeId
    start
    end
    status
    guestName
    reason
    version
  }
}
    `, {"hash":"sha256:49a4c53c98a8985c3fb6794843b286cc9c0742902efc8a528a1344eb5e874658"}) as unknown as TypedDocumentString<UnassignedStaysQuery, UnassignedStaysQueryVariables>;