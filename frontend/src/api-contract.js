const escape = value => String(value ?? '').replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
export function resolveSchema(spec, value) {
  if (!value?.$ref?.startsWith('#/')) return value || {};
  return value.$ref.slice(2).split('/').map(key => key.replace(/~1/g, '/').replace(/~0/g, '~')).reduce((object, key) => object?.[key], spec) || value;
}
export function schemaRows(spec, value, path = '$', required = true, seen = new Set()) {
  if (value?.$ref && seen.has(value.$ref)) return [{path,required,type:value.$ref,description:''}];
  const next = new Set(seen);
  if (value?.$ref) next.add(value.$ref);
  const schema = resolveSchema(spec, value);
  const choices = schema.oneOf || schema.anyOf;
  const constraints = ['enum','const','default','minimum','maximum','minLength','maxLength','format'].filter(key => schema[key] !== undefined).map(key => `${key}=${JSON.stringify(schema[key])}`).join(' · ');
  const row = {path,required,type:[].concat(schema.type || (schema.properties ? 'object' : choices ? 'union' : 'JSON')).join(' | '),description:[schema.description,constraints].filter(Boolean).join(' · ')};
  const rows = [row];
  for (const [name, field] of Object.entries(schema.properties || {})) rows.push(...schemaRows(spec, field, `${path}.${name}`, (schema.required || []).includes(name), next));
  if (schema.items) rows.push(...schemaRows(spec, schema.items, `${path}[*]`, false, next));
  if (schema.additionalProperties && typeof schema.additionalProperties === 'object') rows.push(...schemaRows(spec, schema.additionalProperties, `${path}.{key}`, false, next));
  for (const [index, branch] of (choices || schema.allOf || []).entries()) rows.push(...schemaRows(spec, branch, `${path} [${index+1}]`, required, next));
  return rows;
}
export function renderOperationContract(spec, operation, language = 'en') {
  if (!operation) return '';
  const zh = language.toLowerCase().startsWith('zh');
  const labels = zh ? ['字段','类型','要求','说明','必填','可选','请求字段','响应字段','无响应正文','完整操作契约（JSON）'] : ['Field','Type','Requirement','Description','Required','Optional','Request fields','Response fields','No response body','Complete operation contract (JSON)'];
  const table = (schema, required = true) => `<table class="fields"><thead><tr>${labels.slice(0,4).map(s=>`<th>${s}</th>`).join('')}</tr></thead><tbody>${schemaRows(spec,schema,'$',required).map(r=>`<tr><td><code>${escape(r.path)}</code></td><td>${escape(r.type)}</td><td>${r.required?labels[4]:labels[5]}</td><td>${escape(r.description)}</td></tr>`).join('')}</tbody></table>`;
  const sections = [];
  for (const parameter of operation.parameters || []) {
    const p=resolveSchema(spec,parameter);
    sections.push(`<p><code>${escape(p.in)} · ${escape(p.name)}</code> ${escape(p.description)} (${p.required?labels[4]:labels[5]})</p>${table(p.schema||{},p.required===true)}`);
  }
  const body=resolveSchema(spec,operation.requestBody);
  for (const [media,value] of Object.entries(body.content || {})) sections.push(`<details><summary>${labels[6]} · ${escape(media)}</summary>${table(value.schema||{},body.required===true)}${value.example===undefined?'':`<pre class="ep-example">${escape(JSON.stringify(value.example,null,2))}</pre>`}</details>`);
  for (const [status,response] of Object.entries(operation.responses || {})) {
    const r=resolveSchema(spec,response);
    sections.push(`<details><summary>${labels[7]} · ${escape(status)}</summary><p>${escape(r.description)}</p>${Object.entries(r.content||{}).map(([media,value])=>`<p><code>${escape(media)}</code></p>${table(value.schema||{})}${value.example===undefined?'':`<pre class="ep-example">${escape(JSON.stringify(value.example,null,2))}</pre>`}`).join('')||`<p>${status==='204'?labels[8]:escape(r.description)}</p>`}</details>`);
  }
  sections.push(`<details><summary>${labels[9]}</summary><pre class="ep-example">${escape(JSON.stringify(operation,null,2))}</pre></details>`);
  return `<div class="api-contract">${sections.join('')}</div>`;
}
