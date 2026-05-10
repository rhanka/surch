import { describe, expect, it } from 'vitest';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { gzipSync } from 'node:zlib';
import type { BanDocument } from '$lib/types';
import { MAX_BAN_SUGGESTIONS } from './schema';
import { createBanRepository, createInMemoryBanRepository } from './repository';

const tinyCsv = `id,numero,rep,nom_voie,code_postal,code_insee,nom_commune,lon,lat
75101_0001_00001,1,,Rue de Rivoli,75001,75101,Paris 1er Arrondissement,2.3364,48.8609
33063_0002_00010B,10,B,Cours de l'Intendance,33000,33063,Bordeaux,-0.5792,44.8412
`;

function makeDocument(index: number): BanDocument {
  return {
    city_code: '75101',
    city_name: 'Paris',
    house_number: String(index + 1),
    id: `75101_0001_${String(index + 1).padStart(5, '0')}`,
    label: `${index + 1} Rue de Rivoli 75001 Paris`,
    location: {
      lat: 48.86 + index / 10_000,
      lon: 2.33 + index / 10_000
    },
    postcode: '75001',
    source: 'BAN',
    street_name: 'Rue de Rivoli'
  };
}

describe('BAN repository', () => {
  it('uses ban_tiny as the default offline dataset', async () => {
    const repository = await createBanRepository({ BAN_DATA_DIR: missingBanDataDir() });

    expect(repository.summary()).toMatchObject({
      name: 'ban_tiny',
      documentCount: 3,
      officialSource: 'https://adresse.data.gouv.fr/data/ban/adresses/latest/csv'
    });
    expect(repository.sourceProfile()).toMatchObject({
      kind: 'tiny',
      offline: true,
      officialUrl: 'https://adresse.data.gouv.fr/data/ban/adresses/latest/csv/adresses-75.csv.gz',
      downloadCommand: 'npm run ban:download'
    });
  });

  it('suggests Rue de Rivoli for a contains query on the street label', async () => {
    const repository = await createBanRepository({ BAN_DATA_DIR: missingBanDataDir() });
    const suggestions = repository.suggest({ query: 'rivo' });

    expect(suggestions[0]).toMatchObject({
      id: '75101_0001_00001',
      label: '1 Rue de Rivoli 75001 Paris 1er Arrondissement',
      street_name: 'Rue de Rivoli'
    });
  });

  it('rejects an invalid BAN_CSV_PATH before falling back to tiny data', async () => {
    await expect(
      createBanRepository({ BAN_CSV_PATH: '/tmp/surch-ban-missing.csv' })
    ).rejects.toThrow(/BAN_CSV_PATH/);
  });

  it('loads a gzipped BAN CSV from BAN_CSV_PATH', async () => {
    const dir = mkdtempSync(join(tmpdir(), 'surch-ban-'));
    const path = join(dir, 'adresses-75.csv.gz');
    writeFileSync(path, gzipSync(tinyCsv));

    const repository = await createBanRepository({ BAN_CSV_PATH: path, BAN_SAMPLE_LIMIT: '1' });

    expect(repository.summary()).toMatchObject({
      name: 'adresses-75.csv.gz',
      documentCount: 1
    });
    expect(repository.sourceProfile()).toMatchObject({
      kind: 'csv',
      bounded: true,
      officialUrl: 'https://adresse.data.gouv.fr/data/ban/adresses/latest/csv/adresses-75.csv.gz'
    });
    expect(repository.suggest({ query: 'rivo' })[0].id).toBe('75101_0001_00001');
  });

  it('discovers a downloaded BAN CSV from BAN_DATA_DIR when BAN_CSV_PATH is unset', async () => {
    const dir = mkdtempSync(join(tmpdir(), 'surch-ban-data-'));
    const path = join(dir, 'adresses-75.csv.gz');
    writeFileSync(path, gzipSync(tinyCsv));

    const repository = await createBanRepository({ BAN_DATA_DIR: dir, BAN_SAMPLE_LIMIT: '2' });

    expect(repository.summary()).toMatchObject({
      name: 'adresses-75.csv.gz',
      documentCount: 2
    });
    expect(repository.sourceProfile()).toMatchObject({
      kind: 'csv',
      path,
      offline: false,
      officialUrl: 'https://adresse.data.gouv.fr/data/ban/adresses/latest/csv/adresses-75.csv.gz'
    });
    expect(repository.suggest({ query: 'intendance' })[0].id).toBe('33063_0002_00010B');
  });

  it('reuses an already loaded BAN CSV repository for repeated autocomplete calls', async () => {
    const dir = mkdtempSync(join(tmpdir(), 'surch-ban-cache-'));
    const path = join(dir, 'adresses-75.csv.gz');
    writeFileSync(path, gzipSync(tinyCsv));

    const first = await createBanRepository({ BAN_DATA_DIR: dir, BAN_SAMPLE_LIMIT: '2' });
    const second = await createBanRepository({ BAN_DATA_DIR: dir, BAN_SAMPLE_LIMIT: '2' });

    expect(second).toBe(first);
    expect(second.suggest({ query: '1 rue de rivoli' })[0]).toMatchObject({
      id: '75101_0001_00001',
      street_name: 'Rue de Rivoli'
    });
  });

  it('caps suggestions at the backend maximum', () => {
    const repository = createInMemoryBanRepository({
      documents: Array.from({ length: MAX_BAN_SUGGESTIONS + 5 }, (_, index) => makeDocument(index)),
      source: {
        kind: 'tiny',
        name: 'ban_tiny',
        offline: true,
        bounded: false
      }
    });

    expect(repository.suggest({ query: 'rivoli', limit: 999 })).toHaveLength(MAX_BAN_SUGGESTIONS);
  });
});

function missingBanDataDir(): string {
  return join(tmpdir(), 'surch-ban-data-missing');
}
