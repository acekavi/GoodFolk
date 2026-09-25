/* eslint-disable */
import * as types from './graphql';



/**
 * Map of all GraphQL operations in the project.
 *
 * This map has several performance disadvantages:
 * 1. It is not tree-shakeable, so it will include all operations in the project.
 * 2. It is not minifiable, so the string of a GraphQL query will be multiple times inside the bundle.
 * 3. It does not support dead code elimination, so it will add unused operations.
 *
 * Therefore it is highly recommended to use the babel or swc plugin for production.
 * Learn more about it here: https://the-guild.dev/graphql/codegen/plugins/presets/preset-client#reducing-bundle-size
 */
type Documents = {
    "\n\tquery Inventory($propertyId: UUID!, $from: Date!, $to: Date!) {\n\t\tinventory(propertyId: $propertyId, from: $from, to: $to) {\n\t\t\tdate\n\t\t\troomTypeId\n\t\t\tphysical\n\t\t\tsold\n\t\t\toutOfOrder\n\t\t\tavailable\n\t\t}\n\t\tblocks(propertyId: $propertyId, from: $from, to: $to) {\n\t\t\tid\n\t\t\troomId\n\t\t\tfrom\n\t\t\tto\n\t\t\tkind\n\t\t\treasonId\n\t\t\tnote\n\t\t\tversion\n\t\t}\n\t}\n": typeof types.InventoryDocument,
    "\n\tquery Properties {\n\t\tproperties {\n\t\t\tid\n\t\t\tcode\n\t\t\tname\n\t\t\ttimezone\n\t\t\tbaseCurrency\n\t\t}\n\t}\n": typeof types.PropertiesDocument,
    "\n\tquery RoomTypes($propertyId: UUID!) {\n\t\troomTypes(propertyId: $propertyId) {\n\t\t\tid\n\t\t\tcode\n\t\t\tname\n\t\t\tbaseOccupancy\n\t\t\tmaxAdults\n\t\t\tmaxChildren\n\t\t\tmaxOccupancy\n\t\t\tamenities\n\t\t\tsortOrder\n\t\t\tactive\n\t\t\tversion\n\t\t}\n\t}\n": typeof types.RoomTypesDocument,
    "\n\tquery Rooms($propertyId: UUID!) {\n\t\trooms(propertyId: $propertyId) {\n\t\t\tid\n\t\t\troomTypeId\n\t\t\tnumber\n\t\t\tfloor\n\t\t\tsectionId\n\t\t\tactive\n\t\t\tsortOrder\n\t\t\tversion\n\t\t}\n\t\tsections(propertyId: $propertyId) {\n\t\t\tid\n\t\t\tname\n\t\t\tversion\n\t\t}\n\t\tblockReasons(propertyId: $propertyId) {\n\t\t\tid\n\t\t\tcode\n\t\t\tlabel\n\t\t\tdefaultKind\n\t\t\tactive\n\t\t}\n\t}\n": typeof types.RoomsDocument,
};
const documents: Documents = {
    "\n\tquery Inventory($propertyId: UUID!, $from: Date!, $to: Date!) {\n\t\tinventory(propertyId: $propertyId, from: $from, to: $to) {\n\t\t\tdate\n\t\t\troomTypeId\n\t\t\tphysical\n\t\t\tsold\n\t\t\toutOfOrder\n\t\t\tavailable\n\t\t}\n\t\tblocks(propertyId: $propertyId, from: $from, to: $to) {\n\t\t\tid\n\t\t\troomId\n\t\t\tfrom\n\t\t\tto\n\t\t\tkind\n\t\t\treasonId\n\t\t\tnote\n\t\t\tversion\n\t\t}\n\t}\n": types.InventoryDocument,
    "\n\tquery Properties {\n\t\tproperties {\n\t\t\tid\n\t\t\tcode\n\t\t\tname\n\t\t\ttimezone\n\t\t\tbaseCurrency\n\t\t}\n\t}\n": types.PropertiesDocument,
    "\n\tquery RoomTypes($propertyId: UUID!) {\n\t\troomTypes(propertyId: $propertyId) {\n\t\t\tid\n\t\t\tcode\n\t\t\tname\n\t\t\tbaseOccupancy\n\t\t\tmaxAdults\n\t\t\tmaxChildren\n\t\t\tmaxOccupancy\n\t\t\tamenities\n\t\t\tsortOrder\n\t\t\tactive\n\t\t\tversion\n\t\t}\n\t}\n": types.RoomTypesDocument,
    "\n\tquery Rooms($propertyId: UUID!) {\n\t\trooms(propertyId: $propertyId) {\n\t\t\tid\n\t\t\troomTypeId\n\t\t\tnumber\n\t\t\tfloor\n\t\t\tsectionId\n\t\t\tactive\n\t\t\tsortOrder\n\t\t\tversion\n\t\t}\n\t\tsections(propertyId: $propertyId) {\n\t\t\tid\n\t\t\tname\n\t\t\tversion\n\t\t}\n\t\tblockReasons(propertyId: $propertyId) {\n\t\t\tid\n\t\t\tcode\n\t\t\tlabel\n\t\t\tdefaultKind\n\t\t\tactive\n\t\t}\n\t}\n": types.RoomsDocument,
};

/**
 * The graphql function is used to parse GraphQL queries into a document that can be used by GraphQL clients.
 */
export function graphql(source: "\n\tquery Inventory($propertyId: UUID!, $from: Date!, $to: Date!) {\n\t\tinventory(propertyId: $propertyId, from: $from, to: $to) {\n\t\t\tdate\n\t\t\troomTypeId\n\t\t\tphysical\n\t\t\tsold\n\t\t\toutOfOrder\n\t\t\tavailable\n\t\t}\n\t\tblocks(propertyId: $propertyId, from: $from, to: $to) {\n\t\t\tid\n\t\t\troomId\n\t\t\tfrom\n\t\t\tto\n\t\t\tkind\n\t\t\treasonId\n\t\t\tnote\n\t\t\tversion\n\t\t}\n\t}\n"): typeof import('./graphql').InventoryDocument;
/**
 * The graphql function is used to parse GraphQL queries into a document that can be used by GraphQL clients.
 */
export function graphql(source: "\n\tquery Properties {\n\t\tproperties {\n\t\t\tid\n\t\t\tcode\n\t\t\tname\n\t\t\ttimezone\n\t\t\tbaseCurrency\n\t\t}\n\t}\n"): typeof import('./graphql').PropertiesDocument;
/**
 * The graphql function is used to parse GraphQL queries into a document that can be used by GraphQL clients.
 */
export function graphql(source: "\n\tquery RoomTypes($propertyId: UUID!) {\n\t\troomTypes(propertyId: $propertyId) {\n\t\t\tid\n\t\t\tcode\n\t\t\tname\n\t\t\tbaseOccupancy\n\t\t\tmaxAdults\n\t\t\tmaxChildren\n\t\t\tmaxOccupancy\n\t\t\tamenities\n\t\t\tsortOrder\n\t\t\tactive\n\t\t\tversion\n\t\t}\n\t}\n"): typeof import('./graphql').RoomTypesDocument;
/**
 * The graphql function is used to parse GraphQL queries into a document that can be used by GraphQL clients.
 */
export function graphql(source: "\n\tquery Rooms($propertyId: UUID!) {\n\t\trooms(propertyId: $propertyId) {\n\t\t\tid\n\t\t\troomTypeId\n\t\t\tnumber\n\t\t\tfloor\n\t\t\tsectionId\n\t\t\tactive\n\t\t\tsortOrder\n\t\t\tversion\n\t\t}\n\t\tsections(propertyId: $propertyId) {\n\t\t\tid\n\t\t\tname\n\t\t\tversion\n\t\t}\n\t\tblockReasons(propertyId: $propertyId) {\n\t\t\tid\n\t\t\tcode\n\t\t\tlabel\n\t\t\tdefaultKind\n\t\t\tactive\n\t\t}\n\t}\n"): typeof import('./graphql').RoomsDocument;


export function graphql(source: string) {
  return (documents as any)[source] ?? {};
}
