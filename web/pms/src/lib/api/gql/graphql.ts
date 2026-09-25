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

export type InventoryQueryVariables = Exact<{
  propertyId: string;
  from: string;
  to: string;
}>;


export type InventoryQuery = { inventory: Array<{ date: string, roomTypeId: string, physical: number, sold: number, outOfOrder: number, available: number }>, blocks: Array<{ id: string, roomId: string, from: string, to: string, kind: BlockKind, reasonId: string, note: string, version: number }> };

export type PropertiesQueryVariables = Exact<{ [key: string]: never; }>;


export type PropertiesQuery = { properties: Array<{ id: string, code: string, name: string, timezone: string, baseCurrency: string }> };

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
  }
}
    `) as unknown as TypedDocumentString<PropertiesQuery, PropertiesQueryVariables>;
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