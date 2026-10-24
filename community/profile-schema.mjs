// Shared by the website build and browser. No network access and no code from profiles.
export const MAX_PROFILE_BYTES = 64 * 1024;
export const MAX_DEEP_LINK_BYTES = 16 * 1024;
const PREFERENCES = ['appearance','asr_model','llm_model','threads','context_size','auto_notes','diarization','speaker_count','use_knowledge','use_memory','inference_backend','resource_mode','memory_budget_mib'];
const APPEARANCE = ['mode','theme','accent','background','foreground','font','font_size'];
const bytes = value => new TextEncoder().encode(value).length;
function object(value, allowed, path) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw Error(`${path}: expected an object`);
  for (const key of Object.keys(value)) if (!allowed.includes(key)) throw Error(`${path}: unsupported field ${key}`);
}
function text(value, max, path) {
  if (typeof value !== 'string' || bytes(value) > max || /[\u0000-\u001f\u007f]/.test(value)) throw Error(`${path}: invalid text`);
}
function choice(value, values, path) { if (!values.includes(value)) throw Error(`${path}: unsupported value`); }
function integer(value, min, max, path) { if (!Number.isInteger(value) || value < min || value > max) throw Error(`${path}: outside supported range`); }
export function validateProfile(input, catalog) {
  const raw = typeof input === 'string' ? input : JSON.stringify(input);
  if (bytes(raw) > MAX_PROFILE_BYTES) throw Error('Configuration exceeds 64 KiB');
  const p = JSON.parse(raw);
  object(p, ['schema','name','hardware_hint','preferences'], 'profile');
  if (p.schema !== 1) throw Error('Unsupported configuration schema');
  text(p.name, 160, 'name'); text(p.hardware_hint, 300, 'hardware_hint');
  if (!p.name.trim()) throw Error('Configuration name is empty');
  object(p.preferences, PREFERENCES, 'preferences');
  const v = p.preferences;
  const a = v.appearance === undefined ? {} : v.appearance;
  object(a, APPEARANCE, 'appearance');
  for (const [key, values] of Object.entries({mode:['system','light','dark'],theme:['forest','neutral','slate'],font:['system','serif','mono']})) if (a[key] !== undefined) choice(a[key], values, key);
  for (const key of ['accent','background','foreground']) if (a[key] !== undefined && !(key !== 'accent' && a[key] === '') && !/^#[0-9a-f]{6}$/i.test(a[key])) throw Error(`${key}: use six-digit hex`);
  if (a.font_size !== undefined) integer(a.font_size, 11, 20, 'font_size');
  const models = catalog.models || catalog;
  const asr = models.find(m => m.id === (v.asr_model === undefined ? 'zipformer-small' : v.asr_model));
  const llm = models.find(m => m.id === (v.llm_model === undefined ? 'qwen3-1.7b' : v.llm_model));
  if (asr?.kind !== 'asr' || llm?.kind !== 'llm') throw Error('Unsupported model combination');
  if (v.threads !== undefined) integer(v.threads, 1, 16, 'threads');
  if (v.context_size !== undefined) choice(v.context_size, [2048,4096,8192], 'context_size');
  if (v.speaker_count !== undefined) integer(v.speaker_count, 0, 16, 'speaker_count');
  for (const key of ['auto_notes','diarization','use_knowledge','use_memory']) if (v[key] !== undefined && typeof v[key] !== 'boolean') throw Error(`${key}: expected boolean`);
  const backend = v.inference_backend === undefined ? 'auto' : v.inference_backend;
  choice(backend, ['auto','cpu','metal','mlx'], 'inference_backend');
  if (v.resource_mode !== undefined) choice(v.resource_mode, ['eco','balanced','quality'], 'resource_mode');
  if (v.memory_budget_mib !== undefined && v.memory_budget_mib !== 0) integer(v.memory_budget_mib, 1024, 32768, 'memory_budget_mib');
  if ((llm.format === 'mlx' || llm.id.endsWith('-mlx-4bit')) ? !['auto','mlx'].includes(backend) : backend === 'mlx') throw Error('MLX backend requires a supported MLX model');
  return p;
}
export function deepLink(profile) {
  const raw = JSON.stringify(profile);
  if (bytes(raw) > MAX_PROFILE_BYTES) throw Error('Configuration exceeds 64 KiB');
  const binary = Array.from(new TextEncoder().encode(raw), byte => String.fromCharCode(byte)).join('');
  const encoded = btoa(binary).replaceAll('+','-').replaceAll('/','_').replace(/=+$/, '');
  const url = `omascribe://profile?data=${encoded}`;
  if (bytes(url) > MAX_DEEP_LINK_BYTES) throw Error('Configuration is too large for an app link; download the JSON file instead');
  return url;
}
export function validateSubmission(value, catalog, trusted = false) {
  if (bytes(JSON.stringify(value)) > 128 * 1024) throw Error('Submission exceeds 128 KiB');
  object(value, ['schema','id','origin','author','title','description','platform','memory_gib','profile','benchmark_ids'], 'submission');
  if (value.schema !== 1 || !/^[a-z0-9][a-z0-9-]{2,63}$/.test(value.id)) throw Error('Invalid submission ID or schema');
  choice(value.origin, trusted ? ['official','community'] : ['community'], 'origin');
  text(value.author, 100, 'author'); if (!value.author.trim()) throw Error('Author is empty');
  for (const field of ['title','description']) { object(value[field], ['zh','tw','en'], field); for (const lang of ['zh','tw','en']) text(value[field][lang], field === 'title' ? 200 : 1200, `${field}.${lang}`); }
  choice(value.platform, ['apple-silicon','mac-intel','windows','linux','cross-platform'], 'platform');
  integer(value.memory_gib, 4, 512, 'memory_gib');
  if (!Array.isArray(value.benchmark_ids) || value.benchmark_ids.length > 12 || value.benchmark_ids.some(id => !/^[a-z0-9-]{3,100}$/.test(id))) throw Error('Invalid benchmark references');
  value.profile = validateProfile(value.profile, catalog);
  return value;
}
