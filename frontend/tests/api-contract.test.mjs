import test from 'node:test';
import assert from 'node:assert/strict';
import {renderOperationContract,schemaRows,resolveSchema} from '../src/api-contract.js';
test('nested optional fields, unions, maps and escaped refs preserve their contracts',()=>{
 const spec={components:{schemas:{'A/B':{type:'object',required:['id'],properties:{id:{type:'string'},items:{type:'array',items:{anyOf:[{type:'integer'},{type:'null'}]}}}}}}};
 const rows=schemaRows(spec,{$ref:'#/components/schemas/A~1B'});
 assert.equal(rows.find(r=>r.path==='$.id').required,true);
 assert.equal(rows.find(r=>r.path==='$.items').required,false);
 assert(rows.some(r=>r.type==='null'));
 assert.equal(resolveSchema(spec,{$ref:'#/components/schemas/A~1B'}).type,'object');
 const recursive={components:{schemas:{Node:{type:'object',properties:{child:{$ref:'#/components/schemas/Node'}}}}}};
 assert(schemaRows(recursive,{$ref:'#/components/schemas/Node'}).length<5);
});
test('response docs render all statuses and escape schema content without mutating it',()=>{
 const operation={requestBody:{content:{'application/json':{schema:{type:'object',properties:{'<script>':{type:'string',description:'<img onerror=alert(1)>'}}}}}},responses:{201:{description:'Created',content:{'application/json':{schema:{type:'object',properties:{api_key:{type:'string'}}}}}},204:{description:'Removed'}}};
 const original=JSON.stringify(operation),html=renderOperationContract({},operation,'en');
 assert(html.includes('Response fields · 201'));
 assert(html.includes('No response body'));
 assert(html.includes('$.api_key'));
 assert(!html.includes('<script>'));assert(!html.includes('<img'));
 assert.equal(JSON.stringify(operation),original);
 assert(renderOperationContract({},operation,'zh-CN').includes('响应字段'));
});
test('an optional query value does not become required in the field table',()=>{
 const html=renderOperationContract({}, {parameters:[{in:'query',name:'page',required:false,schema:{type:'integer'}}]},'en');
 assert(html.includes('<code>$</code></td><td>integer</td><td>Optional</td>'));
});
