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
    "\n\tquery Properties {\n\t\tproperties {\n\t\t\tid\n\t\t\tcode\n\t\t\tname\n\t\t\ttimezone\n\t\t\tbaseCurrency\n\t\t\tbusinessDate\n\t\t\tcheckInTime\n\t\t\tcheckOutTime\n\t\t\tversion\n\t\t}\n\t}\n": typeof types.PropertiesDocument,
    "\n\tquery RatePlans($propertyId: UUID!) {\n\t\tratePlans(propertyId: $propertyId) {\n\t\t\tid\n\t\t\tcode\n\t\t\tname\n\t\t\tkind\n\t\t\tsegment\n\t\t\tresidency\n\t\t\tcurrency\n\t\t\tparentId\n\t\t\tdepth\n\t\t\tderiveMode\n\t\t\tderiveValue\n\t\t\troundingStep\n\t\t\textraAdultAmount\n\t\t\tinheritRestrictions\n\t\t\tallowedMealPlans\n\t\t\tcancellationPolicyId\n\t\t\troomTypeIds\n\t\t\tactive\n\t\t\tversion\n\t\t}\n\t\tmealSupplements(propertyId: $propertyId) {\n\t\t\tid\n\t\t\tmealPlan\n\t\t\tcurrency\n\t\t\tadultAmount\n\t\t\tchildAmount\n\t\t\tfrom\n\t\t\tto\n\t\t\tversion\n\t\t}\n\t\tcancellationPolicies(propertyId: $propertyId) {\n\t\t\tid\n\t\t\tname\n\t\t\tversion\n\t\t}\n\t}\n": typeof types.RatePlansDocument,
    "\n\tquery RateGrid($propertyId: UUID!, $ratePlanId: UUID!, $from: Date!, $to: Date!) {\n\t\trateGrid(propertyId: $propertyId, ratePlanId: $ratePlanId, from: $from, to: $to) {\n\t\t\tprices {\n\t\t\t\troomTypeId\n\t\t\t\tdate\n\t\t\t\toccupancy\n\t\t\t\tamount\n\t\t\t}\n\t\t\trestrictions {\n\t\t\t\troomTypeId\n\t\t\t\tdate\n\t\t\t\tclosed\n\t\t\t\tminStay\n\t\t\t\tmaxStay\n\t\t\t\tclosedToArrival\n\t\t\t\tclosedToDeparture\n\t\t\t}\n\t\t}\n\t}\n": typeof types.RateGridDocument,
    "\n\tquery BulkPreview(\n\t\t$propertyId: UUID!\n\t\t$ratePlanId: UUID!\n\t\t$from: Date!\n\t\t$to: Date!\n\t\t$weekdays: [Int!]!\n\t\t$roomTypeIds: [UUID!]!\n\t\t$mode: PriceChangeMode!\n\t\t$value: Int!\n\t) {\n\t\tbulkChangePreview(\n\t\t\tpropertyId: $propertyId\n\t\t\tratePlanId: $ratePlanId\n\t\t\tfrom: $from\n\t\t\tto: $to\n\t\t\tweekdays: $weekdays\n\t\t\troomTypeIds: $roomTypeIds\n\t\t\tmode: $mode\n\t\t\tvalue: $value\n\t\t) {\n\t\t\ttotal\n\t\t\tcells {\n\t\t\t\troomTypeId\n\t\t\t\tdate\n\t\t\t\toccupancy\n\t\t\t\tbefore\n\t\t\t\tafter\n\t\t\t}\n\t\t}\n\t}\n": typeof types.BulkPreviewDocument,
    "\n\tquery Quote(\n\t\t$propertyId: UUID!\n\t\t$roomTypeId: UUID!\n\t\t$ratePlanId: UUID!\n\t\t$mealPlan: MealPlan!\n\t\t$checkIn: Date!\n\t\t$checkOut: Date!\n\t\t$adults: Int!\n\t\t$children: Int!\n\t\t$residency: Residency!\n\t) {\n\t\tquote(\n\t\t\tpropertyId: $propertyId\n\t\t\troomTypeId: $roomTypeId\n\t\t\tratePlanId: $ratePlanId\n\t\t\tmealPlan: $mealPlan\n\t\t\tcheckIn: $checkIn\n\t\t\tcheckOut: $checkOut\n\t\t\tadults: $adults\n\t\t\tchildren: $children\n\t\t\tresidency: $residency\n\t\t) {\n\t\t\tnights {\n\t\t\t\tdate\n\t\t\t\troom\n\t\t\t\tmeal\n\t\t\t}\n\t\t\ttotal\n\t\t\tcurrency\n\t\t\trestrictionsOk\n\t\t\tviolations {\n\t\t\t\tkind\n\t\t\t\tdate\n\t\t\t\tmessage\n\t\t\t}\n\t\t}\n\t}\n": typeof types.QuoteDocument,
    "\n\tquery RoomTypes($propertyId: UUID!) {\n\t\troomTypes(propertyId: $propertyId) {\n\t\t\tid\n\t\t\tcode\n\t\t\tname\n\t\t\tbaseOccupancy\n\t\t\tmaxAdults\n\t\t\tmaxChildren\n\t\t\tmaxOccupancy\n\t\t\tamenities\n\t\t\tsortOrder\n\t\t\tactive\n\t\t\tversion\n\t\t}\n\t}\n": typeof types.RoomTypesDocument,
    "\n\tquery Rooms($propertyId: UUID!) {\n\t\trooms(propertyId: $propertyId) {\n\t\t\tid\n\t\t\troomTypeId\n\t\t\tnumber\n\t\t\tfloor\n\t\t\tsectionId\n\t\t\tactive\n\t\t\tsortOrder\n\t\t\tversion\n\t\t}\n\t\tsections(propertyId: $propertyId) {\n\t\t\tid\n\t\t\tname\n\t\t\tversion\n\t\t}\n\t\tblockReasons(propertyId: $propertyId) {\n\t\t\tid\n\t\t\tcode\n\t\t\tlabel\n\t\t\tdefaultKind\n\t\t\tactive\n\t\t}\n\t}\n": typeof types.RoomsDocument,
};
const documents: Documents = {
    "\n\tquery Inventory($propertyId: UUID!, $from: Date!, $to: Date!) {\n\t\tinventory(propertyId: $propertyId, from: $from, to: $to) {\n\t\t\tdate\n\t\t\troomTypeId\n\t\t\tphysical\n\t\t\tsold\n\t\t\toutOfOrder\n\t\t\tavailable\n\t\t}\n\t\tblocks(propertyId: $propertyId, from: $from, to: $to) {\n\t\t\tid\n\t\t\troomId\n\t\t\tfrom\n\t\t\tto\n\t\t\tkind\n\t\t\treasonId\n\t\t\tnote\n\t\t\tversion\n\t\t}\n\t}\n": types.InventoryDocument,
    "\n\tquery Properties {\n\t\tproperties {\n\t\t\tid\n\t\t\tcode\n\t\t\tname\n\t\t\ttimezone\n\t\t\tbaseCurrency\n\t\t\tbusinessDate\n\t\t\tcheckInTime\n\t\t\tcheckOutTime\n\t\t\tversion\n\t\t}\n\t}\n": types.PropertiesDocument,
    "\n\tquery RatePlans($propertyId: UUID!) {\n\t\tratePlans(propertyId: $propertyId) {\n\t\t\tid\n\t\t\tcode\n\t\t\tname\n\t\t\tkind\n\t\t\tsegment\n\t\t\tresidency\n\t\t\tcurrency\n\t\t\tparentId\n\t\t\tdepth\n\t\t\tderiveMode\n\t\t\tderiveValue\n\t\t\troundingStep\n\t\t\textraAdultAmount\n\t\t\tinheritRestrictions\n\t\t\tallowedMealPlans\n\t\t\tcancellationPolicyId\n\t\t\troomTypeIds\n\t\t\tactive\n\t\t\tversion\n\t\t}\n\t\tmealSupplements(propertyId: $propertyId) {\n\t\t\tid\n\t\t\tmealPlan\n\t\t\tcurrency\n\t\t\tadultAmount\n\t\t\tchildAmount\n\t\t\tfrom\n\t\t\tto\n\t\t\tversion\n\t\t}\n\t\tcancellationPolicies(propertyId: $propertyId) {\n\t\t\tid\n\t\t\tname\n\t\t\tversion\n\t\t}\n\t}\n": types.RatePlansDocument,
    "\n\tquery RateGrid($propertyId: UUID!, $ratePlanId: UUID!, $from: Date!, $to: Date!) {\n\t\trateGrid(propertyId: $propertyId, ratePlanId: $ratePlanId, from: $from, to: $to) {\n\t\t\tprices {\n\t\t\t\troomTypeId\n\t\t\t\tdate\n\t\t\t\toccupancy\n\t\t\t\tamount\n\t\t\t}\n\t\t\trestrictions {\n\t\t\t\troomTypeId\n\t\t\t\tdate\n\t\t\t\tclosed\n\t\t\t\tminStay\n\t\t\t\tmaxStay\n\t\t\t\tclosedToArrival\n\t\t\t\tclosedToDeparture\n\t\t\t}\n\t\t}\n\t}\n": types.RateGridDocument,
    "\n\tquery BulkPreview(\n\t\t$propertyId: UUID!\n\t\t$ratePlanId: UUID!\n\t\t$from: Date!\n\t\t$to: Date!\n\t\t$weekdays: [Int!]!\n\t\t$roomTypeIds: [UUID!]!\n\t\t$mode: PriceChangeMode!\n\t\t$value: Int!\n\t) {\n\t\tbulkChangePreview(\n\t\t\tpropertyId: $propertyId\n\t\t\tratePlanId: $ratePlanId\n\t\t\tfrom: $from\n\t\t\tto: $to\n\t\t\tweekdays: $weekdays\n\t\t\troomTypeIds: $roomTypeIds\n\t\t\tmode: $mode\n\t\t\tvalue: $value\n\t\t) {\n\t\t\ttotal\n\t\t\tcells {\n\t\t\t\troomTypeId\n\t\t\t\tdate\n\t\t\t\toccupancy\n\t\t\t\tbefore\n\t\t\t\tafter\n\t\t\t}\n\t\t}\n\t}\n": types.BulkPreviewDocument,
    "\n\tquery Quote(\n\t\t$propertyId: UUID!\n\t\t$roomTypeId: UUID!\n\t\t$ratePlanId: UUID!\n\t\t$mealPlan: MealPlan!\n\t\t$checkIn: Date!\n\t\t$checkOut: Date!\n\t\t$adults: Int!\n\t\t$children: Int!\n\t\t$residency: Residency!\n\t) {\n\t\tquote(\n\t\t\tpropertyId: $propertyId\n\t\t\troomTypeId: $roomTypeId\n\t\t\tratePlanId: $ratePlanId\n\t\t\tmealPlan: $mealPlan\n\t\t\tcheckIn: $checkIn\n\t\t\tcheckOut: $checkOut\n\t\t\tadults: $adults\n\t\t\tchildren: $children\n\t\t\tresidency: $residency\n\t\t) {\n\t\t\tnights {\n\t\t\t\tdate\n\t\t\t\troom\n\t\t\t\tmeal\n\t\t\t}\n\t\t\ttotal\n\t\t\tcurrency\n\t\t\trestrictionsOk\n\t\t\tviolations {\n\t\t\t\tkind\n\t\t\t\tdate\n\t\t\t\tmessage\n\t\t\t}\n\t\t}\n\t}\n": types.QuoteDocument,
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
export function graphql(source: "\n\tquery Properties {\n\t\tproperties {\n\t\t\tid\n\t\t\tcode\n\t\t\tname\n\t\t\ttimezone\n\t\t\tbaseCurrency\n\t\t\tbusinessDate\n\t\t\tcheckInTime\n\t\t\tcheckOutTime\n\t\t\tversion\n\t\t}\n\t}\n"): typeof import('./graphql').PropertiesDocument;
/**
 * The graphql function is used to parse GraphQL queries into a document that can be used by GraphQL clients.
 */
export function graphql(source: "\n\tquery RatePlans($propertyId: UUID!) {\n\t\tratePlans(propertyId: $propertyId) {\n\t\t\tid\n\t\t\tcode\n\t\t\tname\n\t\t\tkind\n\t\t\tsegment\n\t\t\tresidency\n\t\t\tcurrency\n\t\t\tparentId\n\t\t\tdepth\n\t\t\tderiveMode\n\t\t\tderiveValue\n\t\t\troundingStep\n\t\t\textraAdultAmount\n\t\t\tinheritRestrictions\n\t\t\tallowedMealPlans\n\t\t\tcancellationPolicyId\n\t\t\troomTypeIds\n\t\t\tactive\n\t\t\tversion\n\t\t}\n\t\tmealSupplements(propertyId: $propertyId) {\n\t\t\tid\n\t\t\tmealPlan\n\t\t\tcurrency\n\t\t\tadultAmount\n\t\t\tchildAmount\n\t\t\tfrom\n\t\t\tto\n\t\t\tversion\n\t\t}\n\t\tcancellationPolicies(propertyId: $propertyId) {\n\t\t\tid\n\t\t\tname\n\t\t\tversion\n\t\t}\n\t}\n"): typeof import('./graphql').RatePlansDocument;
/**
 * The graphql function is used to parse GraphQL queries into a document that can be used by GraphQL clients.
 */
export function graphql(source: "\n\tquery RateGrid($propertyId: UUID!, $ratePlanId: UUID!, $from: Date!, $to: Date!) {\n\t\trateGrid(propertyId: $propertyId, ratePlanId: $ratePlanId, from: $from, to: $to) {\n\t\t\tprices {\n\t\t\t\troomTypeId\n\t\t\t\tdate\n\t\t\t\toccupancy\n\t\t\t\tamount\n\t\t\t}\n\t\t\trestrictions {\n\t\t\t\troomTypeId\n\t\t\t\tdate\n\t\t\t\tclosed\n\t\t\t\tminStay\n\t\t\t\tmaxStay\n\t\t\t\tclosedToArrival\n\t\t\t\tclosedToDeparture\n\t\t\t}\n\t\t}\n\t}\n"): typeof import('./graphql').RateGridDocument;
/**
 * The graphql function is used to parse GraphQL queries into a document that can be used by GraphQL clients.
 */
export function graphql(source: "\n\tquery BulkPreview(\n\t\t$propertyId: UUID!\n\t\t$ratePlanId: UUID!\n\t\t$from: Date!\n\t\t$to: Date!\n\t\t$weekdays: [Int!]!\n\t\t$roomTypeIds: [UUID!]!\n\t\t$mode: PriceChangeMode!\n\t\t$value: Int!\n\t) {\n\t\tbulkChangePreview(\n\t\t\tpropertyId: $propertyId\n\t\t\tratePlanId: $ratePlanId\n\t\t\tfrom: $from\n\t\t\tto: $to\n\t\t\tweekdays: $weekdays\n\t\t\troomTypeIds: $roomTypeIds\n\t\t\tmode: $mode\n\t\t\tvalue: $value\n\t\t) {\n\t\t\ttotal\n\t\t\tcells {\n\t\t\t\troomTypeId\n\t\t\t\tdate\n\t\t\t\toccupancy\n\t\t\t\tbefore\n\t\t\t\tafter\n\t\t\t}\n\t\t}\n\t}\n"): typeof import('./graphql').BulkPreviewDocument;
/**
 * The graphql function is used to parse GraphQL queries into a document that can be used by GraphQL clients.
 */
export function graphql(source: "\n\tquery Quote(\n\t\t$propertyId: UUID!\n\t\t$roomTypeId: UUID!\n\t\t$ratePlanId: UUID!\n\t\t$mealPlan: MealPlan!\n\t\t$checkIn: Date!\n\t\t$checkOut: Date!\n\t\t$adults: Int!\n\t\t$children: Int!\n\t\t$residency: Residency!\n\t) {\n\t\tquote(\n\t\t\tpropertyId: $propertyId\n\t\t\troomTypeId: $roomTypeId\n\t\t\tratePlanId: $ratePlanId\n\t\t\tmealPlan: $mealPlan\n\t\t\tcheckIn: $checkIn\n\t\t\tcheckOut: $checkOut\n\t\t\tadults: $adults\n\t\t\tchildren: $children\n\t\t\tresidency: $residency\n\t\t) {\n\t\t\tnights {\n\t\t\t\tdate\n\t\t\t\troom\n\t\t\t\tmeal\n\t\t\t}\n\t\t\ttotal\n\t\t\tcurrency\n\t\t\trestrictionsOk\n\t\t\tviolations {\n\t\t\t\tkind\n\t\t\t\tdate\n\t\t\t\tmessage\n\t\t\t}\n\t\t}\n\t}\n"): typeof import('./graphql').QuoteDocument;
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
