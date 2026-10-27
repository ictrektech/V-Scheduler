export interface DiagnosticEntry {code:string;stage:string;level:'info'|'warning';at:string;count:number}
export function diagnosticSnapshot():DiagnosticEntry[];
export function recordDiagnostic(code:string,stage:string,level?:'info'|'warning'):void;
export function subscribeDiagnostics(listener:(entries:DiagnosticEntry[])=>void):()=>void;
export function clearDiagnostics():void;
export function authDiagnosticCode(error:unknown):string;
export function isSessionError(error:unknown):boolean;
export function userErrorMessage(error:unknown):string;
