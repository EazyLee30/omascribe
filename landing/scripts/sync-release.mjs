import { writeFile, mkdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export function releaseAssetNames(version) {
  const names = {mac_arm:`OmaScribe_${version}_macOS-AppleSilicon.dmg`,mac_intel:`OmaScribe_${version}_macOS-Intel.dmg`,windows:`OmaScribe_${version}_Windows-x64.exe`,checksums:'SHA256SUMS.txt'};
  const [major,minor] = version.split('.').map(Number);
  if (major > 0 || minor >= 3) Object.assign(names, {linux_appimage:`OmaScribe_${version}_Linux-x64.AppImage`,linux_deb:`OmaScribe_${version}_Linux-x64.deb`});
  return names;
}
export function selectRelease(releases) {
  for (const release of [...releases].filter(r=>!r.draft).sort((a,b)=>Number(a.prerelease)-Number(b.prerelease) || Date.parse(b.published_at)-Date.parse(a.published_at))) {
    if (!/^v\d+\.\d+\.\d+(?:-[\w.-]+)?$/.test(release.tag_name)) continue;
    const version = release.tag_name.slice(1);
    const names = releaseAssetNames(version);
    const assets = {};
    for (const [key, name] of Object.entries(names)) {
      const asset = (release.assets || []).find(a=>a.name===name && a.state === 'uploaded');
      const url = `https://github.com/EazyLee30/omascribe/releases/download/${release.tag_name}/${name}`;
      if (!asset || asset.browser_download_url !== url || !Number.isSafeInteger(asset.size) || asset.size <= 0) break;
      assets[key] = {name,size:asset.size,url,digest:asset.digest || null};
    }
    if (Object.keys(assets).length === Object.keys(names).length) return {schema:1,tag:release.tag_name,version,prerelease:release.prerelease,published_at:release.published_at,url:`https://github.com/EazyLee30/omascribe/releases/tag/${release.tag_name}`,assets};
  }
  throw Error('No published release contains all required installers and SHA256SUMS.txt');
}
if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const headers = {'Accept':'application/vnd.github+json','User-Agent':'OmaScribe-site-build'};
  if (process.env.GITHUB_TOKEN) headers.Authorization = `Bearer ${process.env.GITHUB_TOKEN}`;
  const response = await fetch('https://api.github.com/repos/EazyLee30/omascribe/releases?per_page=30', {headers, signal:AbortSignal.timeout(20000)});
  if (!response.ok) throw Error(`GitHub Releases API returned ${response.status}; existing release snapshot was not changed`);
  const manifest = {...selectRelease(await response.json()),fetched_at:new Date().toISOString()};
  await mkdir(resolve(root,'data'),{recursive:true});
  await writeFile(resolve(root,'data/releases.json'),JSON.stringify(manifest,null,2)+'\n');
  console.log(`Verified release ${manifest.tag}: all required installers and checksums`);
}
