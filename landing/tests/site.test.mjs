import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, readdir, stat } from 'node:fs/promises';
import { resolve } from 'node:path';
import { validateProfile, validateSubmission, deepLink } from '../../community/profile-schema.mjs';
import { selectRelease } from '../scripts/sync-release.mjs';
const root=resolve(import.meta.dirname,'../..');
const catalog=JSON.parse(await readFile(resolve(root,'model-catalog.json')));
const preset=JSON.parse(await readFile(resolve(root,'community/profiles/apple-16gb-balanced.json')));
const clone=v=>structuredClone(v);
test('profile round-trip preserves UTF-8 and deep-link has no padding',()=>{
 const p=validateProfile(preset.profile,catalog);p.name='配置：本机 🍃';
 const url=deepLink(p);assert.ok(url.startsWith('omascribe://profile?data='));
 const data=new URL(url).searchParams.get('data');assert.match(data,/^[A-Za-z0-9_-]+$/);
 assert.deepEqual(JSON.parse(Buffer.from(data,'base64url').toString('utf8')),p);
});
test('rejects permissions, custom URLs, commands and unknown keys at every boundary',()=>{
 for(const key of ['network','download_url','command','__proto__','api_key','transcript']){
  const p=clone(preset.profile);Object.defineProperty(p,key,{value:'x',enumerable:true});assert.throws(()=>validateProfile(p,catalog));
  const q=clone(preset.profile);Object.defineProperty(q.preferences,key,{value:'x',enumerable:true});assert.throws(()=>validateProfile(q,catalog));
 }
 const p=clone(preset.profile);p.preferences.appearance={mode:'light',command:'x'};assert.throws(()=>validateProfile(p,catalog));
});
test('bounds, types and model/backend combinations are enforced',()=>{
 for(const [key,value] of [['threads',0],['threads',17],['threads','2'],['context_size',16384],['speaker_count',17],['memory_budget_mib',900],['memory_budget_mib',32769],['inference_backend','gpu'],['resource_mode','unlimited'],['llm_model','../../private'],['asr_model','qwen3-1.7b'],['auto_notes','true'],['appearance',null],['llm_model',null],['inference_backend','']]){
  const p=clone(preset.profile);p.preferences[key]=value;assert.throws(()=>validateProfile(p,catalog),key);
 }
 const p=clone(preset.profile);p.preferences.inference_backend='mlx';assert.throws(()=>validateProfile(p,catalog));
 if(catalog.models.some(m=>m.id==='qwen3-1.7b-mlx-4bit')) {p.preferences.llm_model='qwen3-1.7b-mlx-4bit';assert.doesNotThrow(()=>validateProfile(p,catalog));p.preferences.inference_backend='cpu';assert.throws(()=>validateProfile(p,catalog));}
});
test('file and URL sizes are separate limits; invalid JSON cannot pass',()=>{
 assert.throws(()=>validateProfile('{',catalog));
 assert.throws(()=>validateProfile(' '.repeat(65537),catalog),/64 KiB/);
 assert.throws(()=>deepLink({data:'a'.repeat(13000)}),/too large/);
 const p=clone(preset.profile);p.name='a'.repeat(161);assert.throws(()=>validateProfile(p,catalog));
});
test('community cannot claim official origin and markup remains inert text',()=>{
 assert.throws(()=>validateSubmission(clone(preset),catalog));
 const p=clone(preset);p.origin='community';p.author='<img src=x onerror=alert(1)>';p.profile.name='</script><script>alert(1)</script>';
 const validated=validateSubmission(p,catalog);assert.equal(validated.author,p.author);
 p.author=' ';assert.throws(()=>validateSubmission(p,catalog));
});
function release(version='1.0.0',extra={}){
 const tag=`v${version}`;return {draft:false,prerelease:false,published_at:'2026-10-24T00:00:00Z',tag_name:tag,assets:[`OmaScribe_${version}_macOS-AppleSilicon.dmg`,`OmaScribe_${version}_macOS-Intel.dmg`,`OmaScribe_${version}_Windows-x64.exe`,'SHA256SUMS.txt'].map(name=>({name,state:'uploaded',size:100,browser_download_url:`https://github.com/EazyLee30/omascribe/releases/download/${tag}/${name}`})),...extra};
}
test('release selection supports complete prereleases and skips drafts/partial uploads',()=>{
 const good=release('0.2.0',{prerelease:true});const partial=release('1.0.0',{assets:[],published_at:'2026-10-25T00:00:00Z'});
 assert.equal(selectRelease([partial,release('2.0.0',{draft:true}),good]).tag,'v0.2.0');
 assert.throws(()=>selectRelease([partial]));
 const spoof=release();spoof.assets[0].browser_download_url='https://evil.example/a.dmg';assert.throws(()=>selectRelease([spoof]));
});
test('all 15 localized pages have real internal links and local images',async()=>{
 const dist=resolve(root,'landing/dist');const seen=new Set();
 for(const prefix of ['', 'en/', 'zh-TW/'])for(const page of ['', 'download/', 'models/', 'community/', 'privacy/']){
  const html=await readFile(resolve(dist,prefix,page,'index.html'),'utf8');assert.ok(html.includes('rel="canonical"'));assert.ok(html.includes('aria-current="page"'));assert.ok(!html.includes('undefined'));assert.ok(!html.includes('NaN'));
  for(const match of html.matchAll(/(?:href|src)="(\/[^"?#]*)/g))seen.add(match[1]);
 }
 for(const url of seen){const path=resolve(dist,'.'+url);const info=await stat(path);if(info.isDirectory())await stat(resolve(path,'index.html'));}
});
test('published benchmark numbers are backed by raw data and no inline execution',async()=>{
 const html=await readFile(resolve(root,'landing/dist/models/index.html'),'utf8');assert.ok(!/<script(?![^>]*src=)/.test(html));
 for(const file of await readdir(resolve(root,'docs/benchmarks'))){if(!file.endsWith('.json'))continue;const b=JSON.parse(await readFile(resolve(root,'docs/benchmarks',file)));for(const s of b.summary)assert.ok(html.includes(s.median_generation_tps.toFixed(1)));}
});

test('hosted pages forbid automatic script injection and browser device access',async()=>{
 const headers=await readFile(resolve(root,'landing/dist/_headers'),'utf8');
 assert.match(headers,/Cache-Control:.*no-transform/);
 assert.match(headers,/script-src 'self';/);
 assert.match(headers,/connect-src 'self';/);
 assert.match(headers,/microphone=\(\)/);
 assert.match(headers,/frame-ancestors 'none'/);
});
