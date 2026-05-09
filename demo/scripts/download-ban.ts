import { createWriteStream } from 'node:fs';
import { mkdir, rename, rm, stat } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { Readable } from 'node:stream';
import { pipeline } from 'node:stream/promises';

type Profile = {
  id: 'paris' | 'france';
  label: string;
  fileName: string;
  url: string;
  expectedBytes: number;
};

const baseUrl = 'https://adresse.data.gouv.fr/data/ban/adresses/latest/csv';
const profiles: Record<Profile['id'], Profile> = {
  paris: {
    id: 'paris',
    label: 'BAN Paris departmental sample',
    fileName: 'adresses-75.csv.gz',
    url: `${baseUrl}/adresses-75.csv.gz`,
    expectedBytes: 3_764_435
  },
  france: {
    id: 'france',
    label: 'BAN France full dataset',
    fileName: 'adresses-france.csv.gz',
    url: `${baseUrl}/adresses-france.csv.gz`,
    expectedBytes: 922_089_539
  }
};

const args = process.argv.slice(2);

if (args.includes('--help') || args.includes('-h')) {
  console.log(`Download official BAN CSV data.

Usage:
  npm run ban:download
  npm run ban:download -- --output data/ban/adresses-75.csv.gz
  npm run ban:download -- --profile france

Profiles:
  paris   ${profiles.paris.url}
  france  ${profiles.france.url}
`);
  process.exit(0);
}

const profile = profiles[readArg('--profile') === 'france' ? 'france' : 'paris'];
const output = resolve(readArg('--output') ?? `data/ban/${profile.fileName}`);
const tempOutput = `${output}.tmp`;

await mkdir(dirname(output), { recursive: true });
await rm(tempOutput, { force: true });

const response = await fetch(profile.url);
if (!response.ok || !response.body) {
  throw new Error(`failed to download ${profile.url}: ${response.status} ${response.statusText}`);
}

const contentLength = Number(response.headers.get('content-length'));
if (Number.isFinite(contentLength) && contentLength !== profile.expectedBytes) {
  console.warn(
    `warning: ${profile.fileName} size changed from ${profile.expectedBytes} to ${contentLength} bytes`
  );
}

await pipeline(Readable.fromWeb(response.body), createWriteStream(tempOutput));
await rename(tempOutput, output);

const downloaded = await stat(output);
console.log(`downloaded ${profile.label}`);
console.log(`url: ${profile.url}`);
console.log(`path: ${output}`);
console.log(`bytes: ${downloaded.size}`);
console.log(`BAN_CSV_PATH=${output}`);

function readArg(name: string): string | undefined {
  const index = args.indexOf(name);
  if (index === -1) {
    return undefined;
  }

  return args[index + 1];
}
