/* eslint-disable */
/** Internal type. DO NOT USE DIRECTLY. */
type Exact<T extends { [key: string]: unknown }> = { [K in keyof T]: T[K] };
/** Internal type. DO NOT USE DIRECTLY. */
export type Incremental<T> = T | { [P in keyof T]?: P extends ' $fragmentName' | '__typename' ? T[P] : never };
import type { DocumentTypeDecoration } from '@graphql-typed-document-node/core';
export type AccountKind =
  | 'COMPANY'
  | 'TRAVEL_AGENT';

export type AuditMode =
  | 'AUTO'
  | 'MANUAL';

export type AuditRunStatus =
  | 'COMPLETED'
  | 'FAILED'
  | 'RUNNING';

/** The steps of a close, in the order they run. */
export type AuditStep =
  | 'ADVANCE_BUSINESS_DATE'
  | 'LOCK_FX'
  | 'POST_CHARGES'
  | 'RESOLVE'
  | 'SNAPSHOT_STATS';

export type BlockKind =
  /** Out of inventory: reduces availability. */
  | 'OUT_OF_ORDER'
  /** Still sellable; shown only. */
  | 'OUT_OF_SERVICE';

export type ChangeMode =
  | 'AMOUNT'
  | 'PERCENT';

export type DocumentKind =
  | 'DRIVING_LICENCE'
  | 'NIC'
  | 'OTHER'
  | 'PASSPORT';

/** Whose charges a folio holds: one stay's, an account's (the city ledger) or a reservation's deposits. */
export type FolioKind =
  | 'ACCOUNT'
  | 'DEPOSIT'
  | 'GUEST';

/** An invoiced folio takes no lines or payments until a credit note reopens it. */
export type FolioStatus =
  | 'INVOICED'
  | 'OPEN';

export type IdDocType =
  | 'DRIVING_LICENCE'
  | 'NIC'
  | 'OTHER'
  | 'PASSPORT';

export type ImageFormat =
  | 'AVIF'
  | 'WEBP';

export type InvoiceKind =
  | 'CREDIT_NOTE'
  | 'INVOICE';

/** How urgent a room issue is. */
export type IssueSeverity =
  | 'HIGH'
  | 'LOW'
  | 'MEDIUM';

/** Whether a room issue still needs work. */
export type IssueStatus =
  | 'OPEN'
  | 'RESOLVED';

/** What posted a line. */
export type LineSource =
  | 'ADJUSTMENT'
  | 'CANCELLATION_FEE'
  | 'LAUNDRY'
  | 'MANUAL'
  | 'MEAL_PLAN'
  | 'NO_SHOW_FEE'
  | 'POS'
  | 'ROOM';

export type MealPlan =
  | 'BB'
  | 'FB'
  | 'HB'
  | 'RO';

/** Where an uploaded file is: uploading, processing, then ready or failed. */
export type MediaStatus =
  | 'FAILED'
  | 'PROCESSING'
  | 'READY'
  | 'UPLOADING';

/** Why a stay has no room. */
export type NeedsRoomReason =
  | 'NO_SINGLE_ROOM'
  | 'OVERBOOKED';

/** What a room's stays make of it on the business date. */
export type Occupancy =
  | 'DUE_IN'
  | 'DUE_OUT'
  | 'STAY_OVER'
  /** A guest leaves and another arrives the same day. */
  | 'TURNOVER'
  | 'VACANT';

export type OpsAlertKind =
  | 'AUDIT'
  | 'DRIFT';

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

/** A delivered image's size: `THUMB` is a 320 px square, the others are long edges of 640 to 2400 px. */
export type RenditionSize =
  | 'LG'
  | 'MD'
  | 'SM'
  | 'THUMB'
  | 'XL';

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

/** How ready a room is for a guest. */
export type RoomCondition =
  | 'CLEAN'
  | 'CLEANING'
  | 'DIRTY'
  | 'INSPECTED';

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

export type TaxKind =
  | 'LEVY'
  | 'SERVICE_CHARGE'
  | 'TAX';

/** `ON_BILL` taxes are added to the line's total; `ABSORBED` ones are calculated but borne by the property. */
export type TaxPresentation =
  | 'ABSORBED'
  | 'ON_BILL';

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

export type FoliosQueryVariables = Exact<{
  propertyId: string;
  reservationId?: string | null | undefined;
  accountId?: string | null | undefined;
}>;


export type FoliosQuery = { folios: Array<{ id: string, kind: FolioKind, status: FolioStatus, reservationRoomId: string | null, reservationId: string | null, accountId: string | null, version: number, linesOmitted: boolean, truncated: boolean, lines: Array<{ id: string, source: LineSource, category: string, description: string, quantity: number, net: number, total: number, currency: string, baseTotal: number, businessDate: string, reverses: string | null, reason: string | null, transferId: string | null, postedAt: string, taxes: Array<{ code: string, name: string, rateBp: number, amount: number, presentation: TaxPresentation }> }>, payments: Array<{ id: string, method: string, amount: number, currency: string, baseAmount: number, details: unknown, evidence: string | null, refundOf: string | null, businessDate: string, transferId: string | null, takenAt: string }>, balances: Array<{ currency: string, amount: number, baseAmount: number }>, invoices: Array<{ id: string, kind: InvoiceKind, serial: string, totalLkr: number, issuedAt: string, credits: string | null, pdfReady: boolean }> }> };

export type FolioContextQueryVariables = Exact<{
  propertyId: string;
}>;


export type FolioContextQuery = { propertyTaxSettings: { baseCurrency: string, acceptedCurrencies: Array<string> }, outlets: Array<{ code: string, name: string, active: boolean }> };

export type RoutingQueryVariables = Exact<{
  propertyId: string;
  reservationId: string;
}>;


export type RoutingQuery = { routing: Array<string> };

export type GuestDocumentsQueryVariables = Exact<{
  propertyId: string;
  guestId: string;
}>;


export type GuestDocumentsQuery = { guestDocuments: Array<{ id: string, kind: DocumentKind, status: MediaStatus, mime: string | null, uploadedAt: string, uploadedByName: string | null }> };

export type HousekeepingBoardQueryVariables = Exact<{
  propertyId: string;
}>;


export type HousekeepingBoardQuery = { housekeepingBoard: Array<{ roomId: string, number: string, floor: string | null, sectionId: string | null, roomTypeId: string, condition: RoomCondition, conditionVersion: number, occupancy: Occupancy, blockKind: BlockKind | null, assignee: { userId: string, displayName: string } | null }> };

export type MyRoomsQueryVariables = Exact<{
  propertyId: string;
}>;


export type MyRoomsQuery = { myRooms: Array<{ roomId: string, number: string, roomTypeId: string, condition: RoomCondition, conditionVersion: number, occupancy: Occupancy, openIssues: number, blockKind: BlockKind | null }> };

export type HousekeepersQueryVariables = Exact<{
  propertyId: string;
}>;


export type HousekeepersQuery = { housekeepers: Array<{ userId: string, displayName: string }> };

export type RoomIssuesQueryVariables = Exact<{
  propertyId: string;
  status?: IssueStatus | null | undefined;
}>;


export type RoomIssuesQuery = { roomIssues: Array<{ id: string, roomId: string, severity: IssueSeverity, description: string, status: IssueStatus, reportedAt: string, version: number }> };

export type IssuePhotosQueryVariables = Exact<{
  propertyId: string;
}>;


export type IssuePhotosQuery = { roomIssues: Array<{ id: string, photos: Array<{ assetId: string, status: MediaStatus, error: string | null, caption: string | null, width: number | null, height: number | null, renditions: Array<{ size: RenditionSize, format: ImageFormat, width: number, height: number, url: string }> }> }> };

export type InventoryQueryVariables = Exact<{
  propertyId: string;
  from: string;
  to: string;
}>;


export type InventoryQuery = { inventory: Array<{ date: string, roomTypeId: string, physical: number, sold: number, outOfOrder: number, available: number, sellable: number }>, blocks: Array<{ id: string, roomId: string, from: string, to: string, kind: BlockKind, reasonId: string, note: string, version: number }> };

export type RoomTypeGalleriesQueryVariables = Exact<{
  propertyId: string;
}>;


export type RoomTypeGalleriesQuery = { roomTypes: Array<{ id: string, media: Array<{ assetId: string, status: MediaStatus, error: string | null, caption: string | null, width: number | null, height: number | null, renditions: Array<{ size: RenditionSize, format: ImageFormat, width: number, height: number, url: string }> }> }> };

export type NightAuditQueryVariables = Exact<{
  propertyId: string;
}>;


export type NightAuditQuery = { nightAudit: { businessDate: string, blocking: number, arrivals: Array<{ reservationRoomId: string, reservationId: string, confirmation: string, guest: string, roomNumber: string | null, arrival: string, departure: string, status: RoomStatus, total: number, currency: string, noShowPenalty: number, version: number }>, departures: Array<{ reservationRoomId: string, reservationId: string, confirmation: string, guest: string, roomNumber: string | null, departure: string, version: number, balances: Array<{ currency: string, amount: number }> }>, posBills: Array<{ outlet: string, totalBase: number, bills: Array<{ id: string, billNo: string, billDate: string, currency: string, total: number, totalBase: number, roomNumber: string | null, settled: boolean, highlight: boolean }> }>, bookingBills: { nights: Array<{ reservationRoomId: string, roomNumber: string | null, date: string, roomAmount: number, mealAmount: number, currency: string, problem: string | null, alreadyPosted: number }>, posted: Array<{ category: string, lines: number, totalBase: number }> }, payments: Array<{ method: string, currency: string, count: number, amount: number, baseAmount: number, refunds: number, flagged: Array<string> }> } };

export type AuditRunsQueryVariables = Exact<{
  propertyId: string;
  first: number;
}>;


export type AuditRunsQuery = { auditRuns: Array<{ id: string, businessDate: string, mode: AuditMode, status: AuditRunStatus, startedBy: string | null, attempts: number, error: string | null, failure: unknown, startedAt: string, finishedAt: string | null, steps: Array<{ step: AuditStep, completedAt: string, detail: unknown }> }> };

export type DailyStatsQueryVariables = Exact<{
  propertyId: string;
  from: string;
  to: string;
}>;


export type DailyStatsQuery = { dailyStats: Array<{ businessDate: string, roomsAvailable: number, roomsSold: number, occupancyBp: number, arrivals: number, departures: number, roomRevenue: number, mealPlanRevenue: number, posRevenue: number, feeRevenue: number, otherRevenue: number, adr: number, revpar: number }> };

export type OpsAlertsQueryVariables = Exact<{
  propertyId: string;
}>;


export type OpsAlertsQuery = { opsAlerts: Array<{ kind: OpsAlertKind, message: string, raisedAt: string }> };

export type PropertiesQueryVariables = Exact<{ [key: string]: never; }>;


export type PropertiesQuery = { properties: Array<{ id: string, code: string, name: string, timezone: string, baseCurrency: string, businessDate: string, checkInTime: string, checkOutTime: string, auditTime: string, version: number }> };

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

export type TaxSettingsQueryVariables = Exact<{
  propertyId: string;
}>;


export type TaxSettingsQuery = { propertyTaxSettings: { vatRegistered: boolean, tin: string | null, tdlSmallTurnover: boolean, legalName: string | null, address: string | null, baseCurrency: string, acceptedCurrencies: Array<string>, version: number }, taxRules: Array<{ id: string, code: string, name: string, kind: TaxKind, rateBp: number, appliesTo: Array<string>, base: Array<string>, sequence: number, presentation: TaxPresentation, condition: unknown, effectiveFrom: string, effectiveTo: string | null, version: number }>, outlets: Array<{ code: string, name: string, active: boolean }> };

export type OutletsQueryVariables = Exact<{
  propertyId: string;
}>;


export type OutletsQuery = { outlets: Array<{ id: string, code: string, name: string, serviceCharge: boolean, active: boolean, version: number }> };

export type ExchangeRatesQueryVariables = Exact<{
  propertyId: string;
  from: string;
  to: string;
}>;


export type ExchangeRatesQuery = { fxRates: Array<{ date: string, currency: string, rate: string, source: string, sourceDate: string }>, fxAlert: { message: string, raisedAt: string } | null, propertyTaxSettings: { baseCurrency: string, acceptedCurrencies: Array<string> } };

export type FxAlertQueryVariables = Exact<{
  propertyId: string;
}>;


export type FxAlertQuery = { fxAlert: { message: string, raisedAt: string } | null };

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
export const FoliosDocument = new TypedDocumentString(`
    query Folios($propertyId: UUID!, $reservationId: UUID, $accountId: UUID) {
  folios(
    propertyId: $propertyId
    reservationId: $reservationId
    accountId: $accountId
  ) {
    id
    kind
    status
    reservationRoomId
    reservationId
    accountId
    version
    linesOmitted
    truncated
    lines {
      id
      source
      category
      description
      quantity
      net
      total
      currency
      baseTotal
      businessDate
      reverses
      reason
      transferId
      postedAt
      taxes {
        code
        name
        rateBp
        amount
        presentation
      }
    }
    payments {
      id
      method
      amount
      currency
      baseAmount
      details
      evidence
      refundOf
      businessDate
      transferId
      takenAt
    }
    balances {
      currency
      amount
      baseAmount
    }
    invoices {
      id
      kind
      serial
      totalLkr
      issuedAt
      credits
      pdfReady
    }
  }
}
    `, {"hash":"sha256:7e81d69bc01abf8a3fd16a06bcf45876ea126245231d096ad4d8e22fc45dfe30"}) as unknown as TypedDocumentString<FoliosQuery, FoliosQueryVariables>;
export const FolioContextDocument = new TypedDocumentString(`
    query FolioContext($propertyId: UUID!) {
  propertyTaxSettings(propertyId: $propertyId) {
    baseCurrency
    acceptedCurrencies
  }
  outlets(propertyId: $propertyId) {
    code
    name
    active
  }
}
    `, {"hash":"sha256:599dd5320b801b195ded860016a66bc359b295fa4d42aae2125c0c35647bce54"}) as unknown as TypedDocumentString<FolioContextQuery, FolioContextQueryVariables>;
export const RoutingDocument = new TypedDocumentString(`
    query Routing($propertyId: UUID!, $reservationId: UUID!) {
  routing(propertyId: $propertyId, reservationId: $reservationId)
}
    `, {"hash":"sha256:7a387f46ad872697cd13ef07b4a15fc159b3e3dc107858f565214dd36066e72f"}) as unknown as TypedDocumentString<RoutingQuery, RoutingQueryVariables>;
export const GuestDocumentsDocument = new TypedDocumentString(`
    query GuestDocuments($propertyId: UUID!, $guestId: UUID!) {
  guestDocuments(propertyId: $propertyId, guestId: $guestId) {
    id
    kind
    status
    mime
    uploadedAt
    uploadedByName
  }
}
    `, {"hash":"sha256:2d1c5d5d5c5fadf5222abbbae3c6e843eb0aa5e587f291da49f4be6922fdd472"}) as unknown as TypedDocumentString<GuestDocumentsQuery, GuestDocumentsQueryVariables>;
export const HousekeepingBoardDocument = new TypedDocumentString(`
    query HousekeepingBoard($propertyId: UUID!) {
  housekeepingBoard(propertyId: $propertyId) {
    roomId
    number
    floor
    sectionId
    roomTypeId
    condition
    conditionVersion
    occupancy
    assignee {
      userId
      displayName
    }
    blockKind
  }
}
    `, {"hash":"sha256:e25aabd727d0b329661f3d73dbcda4a8211c9786700938b2730d837fedc83137"}) as unknown as TypedDocumentString<HousekeepingBoardQuery, HousekeepingBoardQueryVariables>;
export const MyRoomsDocument = new TypedDocumentString(`
    query MyRooms($propertyId: UUID!) {
  myRooms(propertyId: $propertyId) {
    roomId
    number
    roomTypeId
    condition
    conditionVersion
    occupancy
    openIssues
    blockKind
  }
}
    `, {"hash":"sha256:543a1e3deb517d6d94cb8c461265a37d9e224306e59939fd482655a78f9cd2ca"}) as unknown as TypedDocumentString<MyRoomsQuery, MyRoomsQueryVariables>;
export const HousekeepersDocument = new TypedDocumentString(`
    query Housekeepers($propertyId: UUID!) {
  housekeepers(propertyId: $propertyId) {
    userId
    displayName
  }
}
    `, {"hash":"sha256:5460be0b73894b27eea037633d372893dee4f9660ccda24a73cb9587f79a4e84"}) as unknown as TypedDocumentString<HousekeepersQuery, HousekeepersQueryVariables>;
export const RoomIssuesDocument = new TypedDocumentString(`
    query RoomIssues($propertyId: UUID!, $status: IssueStatus) {
  roomIssues(propertyId: $propertyId, status: $status) {
    id
    roomId
    severity
    description
    status
    reportedAt
    version
  }
}
    `, {"hash":"sha256:66aaed05b34bb02e4970e9f39bf3aaa4f6a751717e94c00aa0803441d2bd9e38"}) as unknown as TypedDocumentString<RoomIssuesQuery, RoomIssuesQueryVariables>;
export const IssuePhotosDocument = new TypedDocumentString(`
    query IssuePhotos($propertyId: UUID!) {
  roomIssues(propertyId: $propertyId, status: OPEN) {
    id
    photos {
      assetId
      status
      error
      caption
      width
      height
      renditions {
        size
        format
        width
        height
        url
      }
    }
  }
}
    `, {"hash":"sha256:df3e223fe0c919c779edc719cc5f4b76dac1d1d3054930598de10ca5099b1fa3"}) as unknown as TypedDocumentString<IssuePhotosQuery, IssuePhotosQueryVariables>;
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
export const RoomTypeGalleriesDocument = new TypedDocumentString(`
    query RoomTypeGalleries($propertyId: UUID!) {
  roomTypes(propertyId: $propertyId) {
    id
    media {
      assetId
      status
      error
      caption
      width
      height
      renditions {
        size
        format
        width
        height
        url
      }
    }
  }
}
    `, {"hash":"sha256:98199afa545df6979c304af243fc998f32d6e775527f04fbd6b2d1ca1b6e538c"}) as unknown as TypedDocumentString<RoomTypeGalleriesQuery, RoomTypeGalleriesQueryVariables>;
export const NightAuditDocument = new TypedDocumentString(`
    query NightAudit($propertyId: UUID!) {
  nightAudit(propertyId: $propertyId) {
    businessDate
    blocking
    arrivals {
      reservationRoomId
      reservationId
      confirmation
      guest
      roomNumber
      arrival
      departure
      status
      total
      currency
      noShowPenalty
      version
    }
    departures {
      reservationRoomId
      reservationId
      confirmation
      guest
      roomNumber
      departure
      balances {
        currency
        amount
      }
      version
    }
    posBills {
      outlet
      totalBase
      bills {
        id
        billNo
        billDate
        currency
        total
        totalBase
        roomNumber
        settled
        highlight
      }
    }
    bookingBills {
      nights {
        reservationRoomId
        roomNumber
        date
        roomAmount
        mealAmount
        currency
        problem
        alreadyPosted
      }
      posted {
        category
        lines
        totalBase
      }
    }
    payments {
      method
      currency
      count
      amount
      baseAmount
      refunds
      flagged
    }
  }
}
    `, {"hash":"sha256:3b28da9286aa04a2a4e656a69abe59027a3f9d27da56ac7ee4e839c0dafb28d8"}) as unknown as TypedDocumentString<NightAuditQuery, NightAuditQueryVariables>;
export const AuditRunsDocument = new TypedDocumentString(`
    query AuditRuns($propertyId: UUID!, $first: Int!) {
  auditRuns(propertyId: $propertyId, first: $first) {
    id
    businessDate
    mode
    status
    startedBy
    attempts
    error
    failure
    startedAt
    finishedAt
    steps {
      step
      completedAt
      detail
    }
  }
}
    `, {"hash":"sha256:d0d820c99d8ee6fe5ff1782f8fb6ccd4c742a937106bde1880eca763b02fefcc"}) as unknown as TypedDocumentString<AuditRunsQuery, AuditRunsQueryVariables>;
export const DailyStatsDocument = new TypedDocumentString(`
    query DailyStats($propertyId: UUID!, $from: Date!, $to: Date!) {
  dailyStats(propertyId: $propertyId, from: $from, to: $to) {
    businessDate
    roomsAvailable
    roomsSold
    occupancyBp
    arrivals
    departures
    roomRevenue
    mealPlanRevenue
    posRevenue
    feeRevenue
    otherRevenue
    adr
    revpar
  }
}
    `, {"hash":"sha256:6646957a04e5b75a8dd58ba8810587b58bf5270025b336087b084616503c58a7"}) as unknown as TypedDocumentString<DailyStatsQuery, DailyStatsQueryVariables>;
export const OpsAlertsDocument = new TypedDocumentString(`
    query OpsAlerts($propertyId: UUID!) {
  opsAlerts(propertyId: $propertyId) {
    kind
    message
    raisedAt
  }
}
    `, {"hash":"sha256:9dbdff767dda7631d365f8e69c437a04bd601bf99b0c543e5581483c9858c19a"}) as unknown as TypedDocumentString<OpsAlertsQuery, OpsAlertsQueryVariables>;
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
    auditTime
    version
  }
}
    `, {"hash":"sha256:47102d190d3f14bfcac9045a41f0de782abef4c9a351a9a0a8d9ccc99b26c5d8"}) as unknown as TypedDocumentString<PropertiesQuery, PropertiesQueryVariables>;
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
export const TaxSettingsDocument = new TypedDocumentString(`
    query TaxSettings($propertyId: UUID!) {
  propertyTaxSettings(propertyId: $propertyId) {
    vatRegistered
    tin
    tdlSmallTurnover
    legalName
    address
    baseCurrency
    acceptedCurrencies
    version
  }
  taxRules(propertyId: $propertyId) {
    id
    code
    name
    kind
    rateBp
    appliesTo
    base
    sequence
    presentation
    condition
    effectiveFrom
    effectiveTo
    version
  }
  outlets(propertyId: $propertyId) {
    code
    name
    active
  }
}
    `, {"hash":"sha256:44d85f5c6c2b63a1f43b46030699c0d233c251fd28ef2506ac81411d0178d1ef"}) as unknown as TypedDocumentString<TaxSettingsQuery, TaxSettingsQueryVariables>;
export const OutletsDocument = new TypedDocumentString(`
    query Outlets($propertyId: UUID!) {
  outlets(propertyId: $propertyId) {
    id
    code
    name
    serviceCharge
    active
    version
  }
}
    `, {"hash":"sha256:1a3fff7da8b5e4c157994174167fd7d89bf1a31ccb0562ff1de5dfc734ff53be"}) as unknown as TypedDocumentString<OutletsQuery, OutletsQueryVariables>;
export const ExchangeRatesDocument = new TypedDocumentString(`
    query ExchangeRates($propertyId: UUID!, $from: Date!, $to: Date!) {
  fxRates(propertyId: $propertyId, from: $from, to: $to) {
    date
    currency
    rate
    source
    sourceDate
  }
  fxAlert(propertyId: $propertyId) {
    message
    raisedAt
  }
  propertyTaxSettings(propertyId: $propertyId) {
    baseCurrency
    acceptedCurrencies
  }
}
    `, {"hash":"sha256:1b704639f40a4b7613bc546b9fd557d9faffad6239639225955663dfcbc60789"}) as unknown as TypedDocumentString<ExchangeRatesQuery, ExchangeRatesQueryVariables>;
export const FxAlertDocument = new TypedDocumentString(`
    query FxAlert($propertyId: UUID!) {
  fxAlert(propertyId: $propertyId) {
    message
    raisedAt
  }
}
    `, {"hash":"sha256:fb04468ca356a863385a69bf76a4bc4d950f5effbe3b207fbd28086e145ef9b9"}) as unknown as TypedDocumentString<FxAlertQuery, FxAlertQueryVariables>;
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