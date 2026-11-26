/**
 * Verificação de contrato em tempo de compilação.
 *
 * Os tipos em `./generated/` são produzidos a partir das structs Rust por
 * `ts-rs` (`pnpm bindings`). Os schemas zod deste pacote são escritos à mão,
 * porque precisamos de validação em *runtime* na fronteira IPC.
 *
 * Duas fontes descrevendo a mesma coisa divergem silenciosamente — a menos que
 * alguém compare. É o que estes helpers fazem: cada schema zod declara
 * `assertSchemaMatches<Gerado, z.infer<typeof Schema>>()`, e qualquer campo
 * renomeado, adicionado ou removido no Rust quebra `pnpm typecheck`.
 */

/**
 * `true` apenas se `A` e `B` forem exatamente o mesmo tipo.
 *
 * Usa a comparação por identidade de tipos condicionais, que — ao contrário de
 * `extends` — distingue `any`, opcionalidade e union order.
 */
export type Equals<A, B> =
  // O `T` aparece uma vez em cada assinatura de propósito: é a diferença entre
  // as duas assinaturas genéricas que o compilador precisa avaliar, e é
  // exatamente isso que torna a comparação exata. A regra abaixo lê isso como
  // parâmetro supérfluo.
  // eslint-disable-next-line @typescript-eslint/no-unnecessary-type-parameters
  (<T>() => T extends A ? 1 : 2) extends <T>() => T extends B ? 1 : 2 ? true : false;

/**
 * Falha a compilação quando `Generated` e `Inferred` divergem.
 *
 * A mensagem de erro do TypeScript aponta o schema culpado porque a chamada
 * fica ao lado da definição dele.
 */
export function assertSchemaMatches<Generated, Inferred>(
  ..._witness: Equals<Generated, Inferred> extends true
    ? []
    : [error: ContractMismatch<Generated, Inferred>]
): void {
  /* Só existe para o compilador; nada acontece em runtime. */
}

/**
 * Tipo impossível de satisfazer, usado como mensagem de erro legível quando o
 * schema zod e o tipo gerado divergem.
 */
export interface ContractMismatch<Generated, Inferred> {
  readonly __error: 'Schema zod divergiu do tipo gerado do Rust. Rode `pnpm bindings` e ajuste o schema.';
  readonly generated: Generated;
  readonly inferred: Inferred;
}
