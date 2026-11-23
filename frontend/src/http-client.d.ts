export type ApiFailure=Error & {status:number;code:string;requestId:string};
export function apiBases(pathname:string):{gateway:string;scheduler:string;models:string};
export function apiError(payload:any,status:number):ApiFailure;
export function jsonResponse(response:Response):Promise<any>;
export function authenticatedFetch(url:string,init?:RequestInit,auth?:{token?:()=>Promise<string|null|undefined>;clearToken?:(token:string)=>void;fetch?:typeof fetch}):Promise<Response>;

export function retryReadFetch(url:string,init?:RequestInit,fetcher?:typeof fetch):Promise<Response>;
export function isTemporaryReadFailure(error:unknown):boolean;
