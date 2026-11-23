import { makeSecretProviderDefinitionRegistry } from "@legioncode/secret-provider-core/shared/definition";
import * as OnePassword from "@legioncode/secret-provider-onepassword/shared/definition";

export const builtInSecretProviderDefinitions = makeSecretProviderDefinitionRegistry([
  OnePassword.definition,
]);
