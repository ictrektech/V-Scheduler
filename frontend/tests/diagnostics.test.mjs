import {test} from 'node:test';
import assert from 'node:assert/strict';
import {recordDiagnostic,diagnosticSnapshot,clearDiagnostics,userErrorMessage} from '../src/diagnostics.js';
test('diagnostics are bounded, coalesced and contain only whitelisted structured fields',()=>{
 clearDiagnostics();recordDiagnostic('VOS_JWT_EXPIRED','oauth_authorize');recordDiagnostic('VOS_JWT_EXPIRED','oauth_authorize');
 assert.equal(diagnosticSnapshot()[0].count,2);recordDiagnostic('Bearer private-key','oauth_authorize');assert.equal(diagnosticSnapshot().length,1);
 for(let i=0;i<230;i++)recordDiagnostic('TEST_EVENT_'+i,'test');assert.equal(diagnosticSnapshot().length,200);
 assert.deepEqual(Object.keys(diagnosticSnapshot()[0]).sort(),['at','code','count','level','stage']);
});
test('session expiry is quiet while permission and provider configuration failures stay actionable',()=>{
 assert.equal(userErrorMessage(Object.assign(new Error('ExpiredSignature private-token'),{status:401,code:'UNAUTHENTICATED'})),'');
 assert.equal(userErrorMessage(Object.assign(new Error('no access'),{status:403,code:'FORBIDDEN'})),'no access');
 assert.equal(userErrorMessage(Object.assign(new Error('provider key invalid'),{status:401,code:'PROVIDER_AUTH_REQUIRED',sessionError:false})),'provider key invalid');
 assert.ok(!JSON.stringify(diagnosticSnapshot()).includes('private-token'));
});
