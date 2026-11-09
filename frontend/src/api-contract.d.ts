export type ContractSchema = Record<string, unknown>
export function resolveSchema(spec: ContractSchema, value: unknown): ContractSchema
export function schemaRows(spec: ContractSchema, value: unknown): {path:string, required:boolean, type:string, description:string}[]
export function renderOperationContract(spec: ContractSchema, operation: ContractSchema, language?:string): string
