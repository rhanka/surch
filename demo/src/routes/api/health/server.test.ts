import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { GET } from './+server';

const originalBanCsvPath = process.env.BAN_CSV_PATH;
const originalBanDataDir = process.env.BAN_DATA_DIR;
const originalBanSampleLimit = process.env.BAN_SAMPLE_LIMIT;
const tempDirs: string[] = [];

afterEach(() => {
  restoreBanEnv();

  while (tempDirs.length > 0) {
    const dir = tempDirs.pop();
    if (dir) {
      rmSync(dir, { force: true, recursive: true });
    }
  }
});

describe('GET /api/health', () => {
  it('returns demo health without contacting external engines', async () => {
    process.env.BAN_DATA_DIR = missingBanDataDir();

    const response = await GET();
    const body = await response.json();

    expect(response.status).toBe(200);
    expect(body).toMatchObject({
      status: 'ok',
      dataset: 'ban_tiny',
      documentCount: 3
    });
    expect(body.source).toEqual({ kind: 'tiny' });
  });

  it('returns the active BAN CSV dataset when configured', async () => {
    const csvPath = createActiveCsv('adresses-health.csv');
    process.env.BAN_CSV_PATH = csvPath;
    process.env.BAN_SAMPLE_LIMIT = '1';

    const response = await GET();
    const body = await response.json();

    expect(response.status).toBe(200);
    expect(body).toMatchObject({
      status: 'ok',
      dataset: 'adresses-health.csv',
      documentCount: 1
    });
    expect(body.source).toEqual({ kind: 'csv' });
  });
});

function createActiveCsv(fileName: string): string {
  const dir = mkdtempSync(join(tmpdir(), 'surch-ban-health-'));
  tempDirs.push(dir);
  const path = join(dir, fileName);
  writeFileSync(
    path,
    `id,numero,rep,nom_voie,code_postal,code_insee,nom_commune,lon,lat
75101_4242_00012,12,,Rue Health,75001,75101,Paris,2.342,48.859
`
  );

  return path;
}

function restoreBanEnv() {
  if (originalBanCsvPath === undefined) {
    delete process.env.BAN_CSV_PATH;
  } else {
    process.env.BAN_CSV_PATH = originalBanCsvPath;
  }

  if (originalBanDataDir === undefined) {
    delete process.env.BAN_DATA_DIR;
  } else {
    process.env.BAN_DATA_DIR = originalBanDataDir;
  }

  if (originalBanSampleLimit === undefined) {
    delete process.env.BAN_SAMPLE_LIMIT;
  } else {
    process.env.BAN_SAMPLE_LIMIT = originalBanSampleLimit;
  }
}

function missingBanDataDir(): string {
  return join(tmpdir(), 'surch-ban-health-missing');
}
