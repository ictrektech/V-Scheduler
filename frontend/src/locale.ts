export type Locale = 'zh-CN'|'zh-TW'|'en'
export type Preference = Locale|'auto'
export const normalize = (value: unknown):Locale => {const s=String(value||'').replace(/_/g,'-').toLowerCase();return /^zh-(tw|hk|mo|hant)(-|$)/.test(s)?'zh-TW':s.startsWith('zh')?'zh-CN':'en'}
const key='v-scheduler:locale'
const choices=['auto','zh-CN','zh-TW','en']
const vosKey=(name:string|null)=>name==='preferences-locale'||['VIVIBIT-','vben-web-antd-'].some(p=>name?.startsWith(p)&&name.endsWith('-preferences-locale'))
export function createLocaleController(env:{window:any,document:any,navigator:any,storage:any},changed:(value:Locale)=>void){
 const {window:w,document:d,navigator:n,storage:s}=env
 const read=()=>{try{return s.getItem(key)}catch{return null}}
 let preference:Preference=choices.includes(read())?read():'auto',platform:Locale|null=null,revision=0,disposed=false
 let locale:Locale='en',timer:ReturnType<typeof setInterval>|undefined
 function legacy(){if(w.parent===w)return null;try{const keys=Array.from({length:s.length},(_,i)=>s.key(i)).filter(vosKey).sort((a:any,b:any)=>b.localeCompare(a,undefined,{numeric:true}));for(const name of keys){const raw=s.getItem(name);let value=raw;try{const p=JSON.parse(raw);value=p?.value??p}catch{}if(typeof value==='string'&&/^(zh(?:[-_](?:cn|sg|hans|tw|hk|mo|hant))?(?:[-_].*)?|en(?:[-_].*)?)$/i.test(value.trim()))return normalize(value.trim())}}catch{}return null}
 function apply(){locale=preference==='auto'?platform||legacy()||normalize(n.languages?.[0]||n.language):preference;d.documentElement.lang=locale;changed(locale)}
 function setPreference(value:string,persist=true){preference=choices.includes(value)?value as Preference:'auto';if(persist){try{s.setItem(key,preference)}catch{}}apply()}
 function accept(value:unknown){if(!value||disposed)return;platform=normalize(value);if(preference==='auto')apply()}
 async function connect(bridge:any){bridge.broadcast?.on?.('locale-change',(payload:any)=>{revision++;accept(payload?.locale)});const before=revision;try{const snapshot=await bridge.getState?.();if(before===revision)accept(snapshot?.locale)}catch{}}
 const storage=(e:any)=>{if(e.key===key||e.key===null)setPreference(read(),false);else if(vosKey(e.key)&&preference==='auto')apply()}
 const focus=()=>{if(preference==='auto')apply()}
 const language=()=>{if(preference==='auto'&&!platform)apply()}
 w.addEventListener('storage',storage);w.addEventListener('focus',focus);w.addEventListener('languagechange',language)
 apply()
 if(w.vos_platform)void connect(w.vos_platform)
 else if(w.parent!==w){const start=Date.now();timer=setInterval(()=>{if(w.vos_platform){clearInterval(timer);void connect(w.vos_platform)}else if(Date.now()-start>=10000)clearInterval(timer)},50)}
 return {setPreference,get preference(){return preference},get locale(){return locale},dispose(){disposed=true;if(timer)clearInterval(timer);w.removeEventListener('storage',storage);w.removeEventListener('focus',focus);w.removeEventListener('languagechange',language)}}
}
