#!/usr/bin/env node
// Bundle the Lucide icon set (ISC) into the server for the MCP icon tools:
// crates/flexma-server/assets/icons/lucide.json = { version, icons: { name: { svg, tags } }, aliases: { old: name } }.
// Shapes come from web/node_modules/lucide-react; search tags from the matching
// lucide-static release. Run after upgrading lucide-react:
//   node scripts/build-icons.mjs
import { readFileSync, readdirSync, writeFileSync, mkdirSync, copyFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const pkgDir = join(root, 'web/node_modules/lucide-react');
const { version } = JSON.parse(readFileSync(join(pkgDir, 'package.json'), 'utf8'));
const iconDir = join(pkgDir, 'dist/esm/icons');
const outDir = join(root, 'crates/flexma-server/assets/icons');

const tagsRes = await fetch(`https://unpkg.com/lucide-static@${version}/tags.json`);
if (!tagsRes.ok) throw new Error(`couldn't fetch Lucide tags: ${tagsRes.status}`);
const tags = await tagsRes.json();

const attr = (v) => String(v).replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;');
const icons = {};
const aliases = {};
for (const file of readdirSync(iconDir).filter((f) => f.endsWith('.mjs')).sort()) {
  const fileName = file.replace(/\.mjs$/, '');
  // Renamed icons keep their old name as a re-export (home.mjs → house.mjs).
  const reexport = readFileSync(join(iconDir, file), 'utf8').match(/export \{ default \} from '\.\/([a-z0-9-]+)\.mjs'/);
  if (reexport) {
    aliases[fileName] = reexport[1];
    continue;
  }
  const { __iconData } = await import(pathToFileURL(join(iconDir, file)).href);
  if (!__iconData?.name || icons[__iconData.name]) continue;
  const svg = __iconData.node
    .map(([tag, attrs]) => {
      const a = Object.entries(attrs)
        .filter(([k]) => k !== 'key')
        .map(([k, v]) => `${k}="${attr(v)}"`)
        .join(' ');
      return `<${tag} ${a}/>`;
    })
    .join('');
  icons[__iconData.name] = { svg, tags: tags[__iconData.name] ?? [] };
}

mkdirSync(outDir, { recursive: true });
writeFileSync(join(outDir, 'lucide.json'), JSON.stringify({ version, icons, aliases }));
copyFileSync(join(pkgDir, 'LICENSE'), join(outDir, 'LUCIDE-LICENSE'));
console.log(`Wrote ${Object.keys(icons).length} Lucide ${version} icons (+${Object.keys(aliases).length} aliases) to ${outDir}`);
