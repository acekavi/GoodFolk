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
    "\n\tquery Properties {\n\t\tproperties {\n\t\t\tid\n\t\t\tcode\n\t\t\tname\n\t\t\ttimezone\n\t\t\tbaseCurrency\n\t\t}\n\t}\n": typeof types.PropertiesDocument,
};
const documents: Documents = {
    "\n\tquery Properties {\n\t\tproperties {\n\t\t\tid\n\t\t\tcode\n\t\t\tname\n\t\t\ttimezone\n\t\t\tbaseCurrency\n\t\t}\n\t}\n": types.PropertiesDocument,
};

/**
 * The graphql function is used to parse GraphQL queries into a document that can be used by GraphQL clients.
 */
export function graphql(source: "\n\tquery Properties {\n\t\tproperties {\n\t\t\tid\n\t\t\tcode\n\t\t\tname\n\t\t\ttimezone\n\t\t\tbaseCurrency\n\t\t}\n\t}\n"): typeof import('./graphql').PropertiesDocument;


export function graphql(source: string) {
  return (documents as any)[source] ?? {};
}
